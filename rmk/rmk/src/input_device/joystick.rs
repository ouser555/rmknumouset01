use core::cell::Cell;
use core::sync::atomic::{AtomicU16, AtomicU32, Ordering};

use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::signal::Signal;
use rmk_macro::processor;
use usbd_hid::descriptor::MouseReport;

use crate::channel::send_hid_report;
use crate::event::{KeyboardEvent, PointingEvent, PointingProcessorEvent, publish_event_async};
use crate::hid::{HiResScrollReport, JoystickReport, Report};
use crate::input_device::pointing::{ALL_POINTING_DEVICES, MotionAccumulator, PointingMode};
use crate::keymap::KeyMap;
use crate::types::keycode::HidKeyCode;
use crate::RawMutex;

static JOYSTICK_AXES: Mutex<RawMutex, Cell<[i16; 4]>> = Mutex::new(Cell::new([0; 4]));

// Input activity wakes the ADC task without periodically powering the sticks.
pub(crate) static JOYSTICK_WAKE_SIGNAL: Signal<RawMutex, ()> = Signal::new();
pub(crate) static JOYSTICK_WAKE_EPOCH: AtomicU32 = AtomicU32::new(0);
static JOYSTICK_WAKE_DEVICE_ID: AtomicU16 = AtomicU16::new(u16::MAX);

/// Configure the pointing device allowed to wake the sleeping joysticks.
pub(crate) fn configure_joystick_wake(device_id: Option<u8>) {
    JOYSTICK_WAKE_DEVICE_ID.store(device_id.map(u16::from).unwrap_or(u16::MAX), Ordering::Release);
}

fn notify_joystick_activity() {
    JOYSTICK_WAKE_EPOCH.fetch_add(1, Ordering::AcqRel);
    JOYSTICK_WAKE_SIGNAL.signal(());
}

/// Called for keyboard events, including physical keys and encoder turns.
pub(crate) fn notify_keyboard_activity() {
    if JOYSTICK_WAKE_DEVICE_ID.load(Ordering::Acquire) != u16::MAX {
        notify_joystick_activity();
    }
}

/// Called only after a pointing sensor reports actual movement.
pub(crate) fn notify_pointing_motion(device_id: u8) {
    if JOYSTICK_WAKE_DEVICE_ID.load(Ordering::Acquire) == u16::from(device_id) {
        notify_joystick_activity();
    }
}

#[derive(Clone, Copy)]
struct DigitalSources {
    wasd: [[bool; 4]; 2],
}

static DIGITAL_SOURCES: Mutex<RawMutex, Cell<DigitalSources>> = Mutex::new(Cell::new(DigitalSources {
    wasd: [[false; 4]; 2],
}));

fn digital_source_slot(device_id: u8) -> Option<usize> {
    match device_id {
        0 => Some(0),
        2 => Some(1),
        _ => None,
    }
}

fn update_combined_wasd_directions(device_id: u8, next: [bool; 4]) -> ([bool; 4], [bool; 4]) {
    DIGITAL_SOURCES.lock(|cell| {
        let mut sources = cell.get();
        let states = &mut sources.wasd;
        let before = core::array::from_fn(|direction| states[0][direction] || states[1][direction]);
        if let Some(slot) = digital_source_slot(device_id) {
            states[slot] = next;
        }
        let after = core::array::from_fn(|direction| states[0][direction] || states[1][direction]);
        cell.set(sources);
        (before, after)
    })
}

fn combined_joystick_report(device_id: u8, buttons: u8, axes: [i16; 2]) -> JoystickReport {
    JOYSTICK_AXES.lock(|state| {
        let mut values = state.get();
        let base = if device_id == 0 { 0 } else if device_id == 2 { 2 } else { return JoystickReport::default() };
        values[base..base + 2].copy_from_slice(&axes);
        state.set(values);
        JoystickReport {
            buttons,
            x: values[0],
            y: values[1],
            rx: values[2],
            ry: values[3],
        }
    })
}

#[derive(Clone, Copy, Debug)]
pub struct JoystickPowerConfig {
    pub polling_rate_hz: u16,
    pub idle_polling_rate_hz: u16,
    pub sample_settle_us: u32,
    pub boot_settle_ms: u32,
    /// Zero retains the existing active/idle polling behavior.
    pub sleep_after_ms: u32,
    pub wake_device_id: Option<u8>,
}

impl JoystickPowerConfig {
    pub(crate) fn period_us(&self, idle: bool) -> u64 {
        let hz = if idle {
            self.idle_polling_rate_hz
        } else {
            self.polling_rate_hz
        };
        1_000_000 / u64::from(hz.max(1))
    }
}

#[derive(Default)]
pub(crate) struct IdleTracker {
    centered_since_us: Option<u64>,
    pub idle: bool,
}

impl IdleTracker {
    pub(crate) fn observe(&mut self, now_us: u64, centered: bool) {
        if !centered {
            self.centered_since_us = None;
            self.idle = false;
        } else {
            let since = *self.centered_since_us.get_or_insert(now_us);
            self.idle = now_us.saturating_sub(since) >= 1_200_000;
        }
    }
}

pub(crate) fn adc_axis(raw: i16) -> i16 {
    (raw + i16::MIN / 2).saturating_mul(2)
}

pub(crate) fn apply_deadzone(value: i16, deadzone: u16) -> i16 {
    let value = i32::from(value);
    let magnitude = (value.abs() - i32::from(deadzone)).max(0);
    (value.signum() * magnitude) as i16
}

const NRF_SAADC_12_BIT_MAX: i16 = 4095;
const JOYSTICK_LOGICAL_MAX: i32 = 32767;
const DIGITAL_PRESS_THRESHOLD: i16 = 9000;
const DIGITAL_RELEASE_THRESHOLD: i16 = 6500;
const WASD_KEYS: [HidKeyCode; 4] = [HidKeyCode::W, HidKeyCode::A, HidKeyCode::S, HidKeyCode::D];
// Dedicated Vial-only row mirrors QMK numouse: up, left, right, down.
// Internal direction order is up, left, down, right.
const USER_WASD_POSITIONS: [[(u8, u8); 4]; 2] = [
    [(1, 0), (1, 1), (1, 3), (1, 2)],
    [(2, 0), (2, 1), (2, 3), (2, 2)],
];

fn user_wasd_positions(device_id: u8) -> [(u8, u8); 4] {
    USER_WASD_POSITIONS[if device_id == 2 { 1 } else { 0 }]
}

fn spacemouse_axis_selection(x: i16, y: i16) -> (i16, bool) {
    let x_dominant = x.unsigned_abs() >= y.unsigned_abs();
    (if x_dominant { x } else { 0 }, !x_dominant)
}

fn digital_axis_state(value: i16, negative_held: bool, positive_held: bool) -> (bool, bool) {
    let negative_threshold = if negative_held {
        DIGITAL_RELEASE_THRESHOLD
    } else {
        DIGITAL_PRESS_THRESHOLD
    };
    let positive_threshold = if positive_held {
        DIGITAL_RELEASE_THRESHOLD
    } else {
        DIGITAL_PRESS_THRESHOLD
    };
    (value <= -negative_threshold, value >= positive_threshold)
}

fn digital_directions(x: i16, y: i16, held: [bool; 4]) -> [bool; 4] {
    let (up, down) = digital_axis_state(y, held[0], held[2]);
    let (left, right) = digital_axis_state(x, held[1], held[3]);
    [up, left, down, right]
}

fn normalize_joystick_input(value: i16, bias: i16, deadzone: u16) -> i16 {
    let bound = if value < 0 {
        i32::from(apply_deadzone(i16::MIN.saturating_add(bias), deadzone)).abs()
    } else {
        i32::from(apply_deadzone(
            adc_axis(NRF_SAADC_12_BIT_MAX).saturating_add(bias),
            deadzone,
        ))
        .abs()
    };
    if bound == 0 {
        return 0;
    }
    (i32::from(value) * JOYSTICK_LOGICAL_MAX / bound).clamp(-JOYSTICK_LOGICAL_MAX, JOYSTICK_LOGICAL_MAX) as i16
}

fn joystick_axes<const N: usize>(
    centered: &[i16; N],
    bias: &[i16; N],
    deadzone: u16,
    transform: &[[i16; N]; N],
) -> [i16; N] {
    let normalized: [i16; N] = core::array::from_fn(|i| normalize_joystick_input(centered[i], bias[i], deadzone));
    core::array::from_fn(|output| {
        let mut weighted = 0i32;
        let mut weight_sum = 0i32;
        for (input, divisor) in transform[output].iter().enumerate() {
            if *divisor == 0 {
                continue;
            }
            // The cursor transform stores divisors. Their inverse is the axis
            // weight; fixed point keeps the joystick's analog resolution.
            let weight = 32_768 / i32::from(divisor.unsigned_abs()).max(1);
            weighted += i32::from(normalized[input]) * weight * i32::from(divisor.signum());
            weight_sum += weight;
        }
        if weight_sum == 0 {
            0
        } else {
            (weighted / weight_sum).clamp(-JOYSTICK_LOGICAL_MAX, JOYSTICK_LOGICAL_MAX) as i16
        }
    })
}

#[processor(subscribe = [PointingEvent, PointingProcessorEvent])]
pub struct JoystickProcessor<'a, const N: usize> {
    idle_report_filter: idle_report_filter::IdleReportFilter,
    /// Only process events from this device id. Use ALL_POINTING_DEVICES (255) to accept all.
    device_id: u8,
    transform: [[i16; N]; N],
    bias: [i16; N],
    keymap: &'a KeyMap<'a>,
    record: [i16; N],
    resolution: u16,
    deadzone: u16,
    accumulator: MotionAccumulator,
    current_mode: PointingMode,
    digital_state: [bool; 4],
}

mod idle_report_filter {
    /// Suppress repeated stationary reports, never repeated relative movement.
    #[derive(Default)]
    pub(super) struct IdleReportFilter {
        last_rest: Option<(u32, u8)>,
    }

    impl IdleReportFilter {
        pub(super) fn should_send(&self, connection_epoch: u32, buttons: u8, moving: bool) -> bool {
            moving || self.last_rest != Some((connection_epoch, buttons))
        }

        pub(super) fn record_queued(&mut self, connection_epoch: u32, buttons: u8, moving: bool) {
            self.last_rest = (!moving).then_some((connection_epoch, buttons));
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn idle_filter_preserves_motion_buttons_and_reconnection() {
            let mut filter = IdleReportFilter::default();
            assert!(filter.should_send(0, 0, false));
            filter.record_queued(0, 0, false);
            assert!(!filter.should_send(0, 0, false));
            assert!(filter.should_send(0, 0, true));
            filter.record_queued(0, 0, true);
            assert!(filter.should_send(0, 0, true));
            assert!(filter.should_send(0, 0, false));
            filter.record_queued(0, 0, false);
            assert!(filter.should_send(0, 1, false));
            filter.record_queued(0, 1, false);
            assert!(!filter.should_send(0, 1, false));
            assert!(filter.should_send(0, 0, false));
            filter.record_queued(0, 0, false);
            assert!(filter.should_send(1, 0, false));
            // Discarding a cache while disconnected requires a fresh rest report.
            filter = IdleReportFilter::default();
            assert!(filter.should_send(0, 0, false));
        }
    }
}

impl<'a, const N: usize> JoystickProcessor<'a, N> {
    pub fn new(
        device_id: u8,
        transform: [[i16; N]; N],
        bias: [i16; N],
        resolution: u16,
        keymap: &'a KeyMap<'a>,
    ) -> Self {
        Self {
            idle_report_filter: idle_report_filter::IdleReportFilter::default(),
            device_id,
            transform,
            bias,
            resolution,
            keymap,
            record: [0; N],
            deadzone: 0,
            accumulator: MotionAccumulator::default(),
            current_mode: PointingMode::default(),
            digital_state: [false; 4],
        }
    }

    pub fn with_deadzone(mut self, deadzone: u16) -> Self {
        self.deadzone = deadzone;
        self
    }

    /// Set the output mode used for subsequent joystick samples.
    pub fn set_pointing_mode(&mut self, mode: PointingMode) -> &mut Self {
        if self.current_mode != mode {
            self.accumulator.reset();
            self.idle_report_filter = idle_report_filter::IdleReportFilter::default();
        }
        self.current_mode = mode;
        self
    }

    async fn update_digital_keys(&mut self, x: i16, y: i16, user_defined: bool) {
        let next = digital_directions(x, y, self.digital_state);
        let (before, after) = if user_defined {
            (self.digital_state, next)
        } else {
            update_combined_wasd_directions(self.device_id, next)
        };
        let positions = user_wasd_positions(self.device_id);
        for direction in 0..4 {
            if before[direction] == after[direction] {
                continue;
            }
            let event = if user_defined {
                let (row, col) = positions[direction];
                KeyboardEvent::key(row, col, after[direction])
            } else {
                KeyboardEvent::virtual_key(WASD_KEYS[direction], after[direction])
            };
            publish_event_async(event).await;
        }
        self.digital_state = next;
    }

    async fn release_digital_keys(&mut self, user_defined: bool) {
        let (before, after) = if user_defined {
            (self.digital_state, [false; 4])
        } else {
            update_combined_wasd_directions(self.device_id, [false; 4])
        };
        let positions = user_wasd_positions(self.device_id);
        for direction in 0..4 {
            if before[direction] == after[direction] {
                continue;
            }
            let event = if user_defined {
                let (row, col) = positions[direction];
                KeyboardEvent::key(row, col, after[direction])
            } else {
                KeyboardEvent::virtual_key(WASD_KEYS[direction], after[direction])
            };
            publish_event_async(event).await;
        }
        self.digital_state = [false; 4];
    }

    async fn on_pointing_event(&mut self, event: PointingEvent) {
        if self.device_id != ALL_POINTING_DEVICES && event.device_id != self.device_id {
            return;
        }
        for (rec, e) in self.record.iter_mut().zip(event.axes.iter()) {
            *rec = e.value;
        }
        debug!("Joystick info: {:#?}", self.record);
        self.generate_report().await;
    }

    async fn generate_report(&mut self) {
        let mut report = [0i16; N];

        debug!("JoystickProcessor::generate_report: record = {:?}", self.record);
        for (rec, b) in self.record.iter_mut().zip(self.bias.iter()) {
            *rec = apply_deadzone(rec.saturating_add(*b), self.deadzone);
        }
        let joystick_report = joystick_axes(&self.record, &self.bias, self.deadzone, &self.transform);

        for (rep, transform) in report.iter_mut().zip(self.transform.iter()) {
            for (w, v) in transform.iter().zip(self.record) {
                if *w == 0 {
                    // ignore zero weight
                    continue;
                }
                *rep = rep.saturating_add(v.saturating_div(*w));
                *rep = *rep - *rep % self.resolution as i16;
            }
        }

        debug!("JoystickProcessor::generate_report: report = {:?}", report);
        let buttons = self.keymap.mouse_buttons();
        let x = report.first().copied().unwrap_or(0);
        let y = report.get(1).copied().unwrap_or(0);
        let z = report.get(2).copied().unwrap_or(0);
        let joystick_x = joystick_report.first().copied().unwrap_or(0);
        let joystick_y = joystick_report.get(1).copied().unwrap_or(0);

        if matches!(self.current_mode, PointingMode::Wasd | PointingMode::UserWasd) {
            self.update_digital_keys(
                joystick_x,
                joystick_y,
                matches!(self.current_mode, PointingMode::UserWasd),
            )
            .await;
            return;
        }

        // Do not remember reports dropped while no host is selected.
        let epoch = crate::state::CONNECTION_EPOCH.lock(|epoch| epoch.get());
        if crate::state::active_transport().is_none() {
            self.idle_report_filter = idle_report_filter::IdleReportFilter::default();
            return;
        }
        let moving = if matches!(self.current_mode, PointingMode::Joystick | PointingMode::SpaceMouse) {
            joystick_x != 0 || joystick_y != 0
        } else {
            x != 0 || y != 0 || z != 0
        };
        if !self.idle_report_filter.should_send(epoch, buttons, moving) {
            return;
        }

        match self.current_mode {
            PointingMode::Cursor(cursor_config) => {
                let x = x.saturating_mul(i16::from(cursor_config.multiplier_x));
                let y = y.saturating_mul(i16::from(cursor_config.multiplier_y));
                let x = if cursor_config.invert_x { -x } else { x };
                let y = if cursor_config.invert_y { -y } else { y };
                send_hid_report(Report::MouseReport(MouseReport {
                    buttons,
                    x: x.clamp(i8::MIN as i16, i8::MAX as i16) as i8,
                    y: y.clamp(i8::MIN as i16, i8::MAX as i16) as i8,
                    wheel: 0,
                    pan: 0,
                }))
                .await;
            }
            PointingMode::Sniper(cfg) => {
                let (x, y) =
                    self.accumulator
                        .accumulate(x, y, (cfg.multiplier, cfg.divisor), (cfg.multiplier, cfg.divisor));
                send_hid_report(Report::MouseReport(MouseReport {
                    buttons,
                    x: (if cfg.invert_x { -x } else { x }).clamp(i8::MIN as i16, i8::MAX as i16) as i8,
                    y: (if cfg.invert_y { -y } else { y }).clamp(i8::MIN as i16, i8::MAX as i16) as i8,
                    wheel: 0,
                    pan: 0,
                }))
                .await;
            }
            PointingMode::Scroll(cfg) => {
                let (x, y) = self.accumulator.accumulate(
                    x,
                    y,
                    (cfg.multiplier_x, cfg.divisor_x),
                    (cfg.multiplier_y, cfg.divisor_y),
                );
                send_hid_report(Report::MouseReport(MouseReport {
                    buttons,
                    x: 0,
                    y: 0,
                    wheel: (if cfg.invert_y { y } else { -y }).clamp(i8::MIN as i16, i8::MAX as i16) as i8,
                    pan: (if cfg.invert_x { -x } else { x }).clamp(i8::MIN as i16, i8::MAX as i16) as i8,
                }))
                .await;
            }
            PointingMode::HiResScroll(hi_res_config) => {
                let cfg = hi_res_config.scroll;
                let (x, y) = self.accumulator.accumulate(
                    x,
                    y,
                    (cfg.multiplier_x, cfg.divisor_x),
                    (cfg.multiplier_y, cfg.divisor_y),
                );
                let units = i16::from(hi_res_config.units_per_step.max(1));
                send_hid_report(Report::HiResScrollReport(HiResScrollReport {
                    x: 0,
                    y: 0,
                    wheel: (if cfg.invert_y { y } else { -y }).saturating_mul(units),
                    pan: (if cfg.invert_x { -x } else { x }).saturating_mul(units),
                }))
                .await;
            }
            PointingMode::Joystick => {
                send_hid_report(Report::JoystickReport(combined_joystick_report(
                    self.device_id,
                    buttons,
                    [joystick_x, joystick_y],
                )))
                .await;
            }
            PointingMode::SpaceMouse => {
                let (left_x, scroll_active) = spacemouse_axis_selection(joystick_x, joystick_y);
                send_hid_report(Report::JoystickReport(combined_joystick_report(
                    self.device_id,
                    buttons,
                    [left_x, 0],
                )))
                .await;

                if scroll_active {
                    let (_, scroll_y) = self.accumulator.accumulate(0, y, (0, 0), (1, 5));
                    if scroll_y != 0 {
                        send_hid_report(Report::HiResScrollReport(HiResScrollReport {
                            x: 0,
                            y: 0,
                            wheel: -scroll_y,
                            pan: 0,
                        }))
                        .await;
                    }
                } else {
                    self.accumulator.reset_y();
                }
            }
            PointingMode::Caret(_) => {}
            PointingMode::Wasd | PointingMode::UserWasd => unreachable!(),
        }
        // A transport change can abort queueing while this task is waiting for capacity.
        if crate::state::CONNECTION_EPOCH.lock(|current| current.get()) == epoch {
            self.idle_report_filter.record_queued(epoch, buttons, moving);
        }
    }

    async fn on_pointing_processor_event(&mut self, event: PointingProcessorEvent) {
        if self.device_id == ALL_POINTING_DEVICES || self.device_id == event.device_id {
            if matches!(self.current_mode, PointingMode::Wasd | PointingMode::UserWasd)
                && self.current_mode != event.mode
            {
                self.release_digital_keys(matches!(self.current_mode, PointingMode::UserWasd))
                    .await;
            }
            if matches!(self.current_mode, PointingMode::Joystick | PointingMode::SpaceMouse)
                && !matches!(event.mode, PointingMode::Joystick | PointingMode::SpaceMouse)
            {
                send_hid_report(Report::JoystickReport(combined_joystick_report(
                    self.device_id,
                    self.keymap.mouse_buttons(),
                    [0; 2],
                )))
                .await;
            }
            self.set_pointing_mode(event.mode);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deadzone_is_symmetric_and_continuous() {
        for value in -100..=100 {
            assert_eq!(apply_deadzone(value, 100), 0);
        }
        assert_eq!(apply_deadzone(101, 100), 1);
        assert_eq!(apply_deadzone(-101, 100), -1);
        assert_eq!(apply_deadzone(i16::MIN, 100), -32668);
    }

    #[test]
    fn idle_requires_continuous_centering_and_wakes_immediately() {
        let mut tracker = IdleTracker::default();
        tracker.observe(0, true);
        tracker.observe(1_199_999, true);
        assert!(!tracker.idle);
        tracker.observe(1_200_000, true);
        assert!(tracker.idle);
        tracker.observe(1_200_001, false);
        assert!(!tracker.idle);
    }

    #[test]
    fn polling_period_follows_idle_state() {
        let config = JoystickPowerConfig {
            polling_rate_hz: 50,
            idle_polling_rate_hz: 5,
            sample_settle_us: 3,
            boot_settle_ms: 2,
        };
        assert_eq!(config.period_us(false), 20_000);
        assert_eq!(config.period_us(true), 200_000);
    }

    #[test]
    fn joystick_mode_uses_full_hid_range_without_cursor_quantization() {
        let bias = [29130, 29365];
        let transform = [[80, 0], [0, 80]];
        let low = [
            apply_deadzone(i16::MIN.saturating_add(bias[0]), 400),
            apply_deadzone(i16::MIN.saturating_add(bias[1]), 400),
        ];
        let high = [
            apply_deadzone(adc_axis(NRF_SAADC_12_BIT_MAX).saturating_add(bias[0]), 400),
            apply_deadzone(adc_axis(NRF_SAADC_12_BIT_MAX).saturating_add(bias[1]), 400),
        ];

        assert_eq!(joystick_axes(&low, &bias, 400, &transform), [-32767, -32767]);
        assert_eq!(joystick_axes(&high, &bias, 400, &transform), [32767, 32767]);
        assert_eq!(joystick_axes(&[0, 0], &bias, 400, &transform), [0, 0]);
        assert_ne!(joystick_axes(&[1, 1], &bias, 400, &transform), [0, 0]);
    }

    #[test]
    fn user_wasd_rows_are_independent() {
        assert_eq!(user_wasd_positions(0), [(1, 0), (1, 1), (1, 3), (1, 2)]);
        assert_eq!(user_wasd_positions(2), [(2, 0), (2, 1), (2, 3), (2, 2)]);
    }

    #[test]
    fn two_joysticks_fill_independent_hid_axes() {
        JOYSTICK_AXES.lock(|state| state.set([0; 4]));
        let first = combined_joystick_report(0, 0, [101, -102]);
        assert_eq!((first.x, first.y), (101, -102));
        assert_eq!((first.rx, first.ry), (0, 0));

        let both = combined_joystick_report(2, 0, [-201, 202]);
        assert_eq!((both.x, both.y), (101, -102));
        assert_eq!((both.rx, both.ry), (-201, 202));
    }

    #[test]
    fn spacemouse_uses_qmk_dominant_axis_selection() {
        assert_eq!(spacemouse_axis_selection(12_000, 4_000), (12_000, false));
        assert_eq!(spacemouse_axis_selection(4_000, -12_000), (0, true));
        assert_eq!(spacemouse_axis_selection(-8_000, 8_000), (-8_000, false));
    }

    #[test]
    fn digital_modes_support_diagonals_and_release_hysteresis() {
        assert_eq!(
            digital_directions(10_000, -10_000, [false; 4]),
            [true, false, false, true]
        );
        assert_eq!(
            digital_directions(7_000, -7_000, [true, false, false, true]),
            [true, false, false, true]
        );
        assert_eq!(
            digital_directions(6_000, -6_000, [true, false, false, true]),
            [false; 4]
        );
    }
}
