use core::sync::atomic::{AtomicBool, Ordering};

use embassy_futures::select::{select, Either};
use embassy_time::{Duration, Instant, Timer};
use embedded_hal::digital::OutputPin;

use super::{DisplayDriver, DisplayRenderer, RenderContext};
use crate::core_traits::Runnable;
use crate::event::{EventSubscriber, PointingProcessorEvent, SubscribableEvent};
use crate::input_device::pointing::PointingMode;
use crate::keymap::KeyMap;

static POINTING_MODE_DISPLAY_AWAKE: AtomicBool = AtomicBool::new(false);

/// Return the actual state maintained by the pointing-mode display task.
pub fn pointing_mode_display_is_awake() -> bool {
    POINTING_MODE_DISPLAY_AWAKE.load(Ordering::Acquire)
}

/// OLED processor dedicated to showing pointing-device mode changes.
///
/// A mode event wakes the display and restarts its idle deadline. Other
/// keyboard activity deliberately does not wake it.
pub struct PointingModeDisplayProcessor<'a, D, R, P>
where
    D: DisplayDriver,
    R: DisplayRenderer<D::Color>,
    P: OutputPin,
{
    keymap: &'a KeyMap<'a>,
    display: D,
    renderer: R,
    /// Retain ownership of the asserted OLED supply pin for the task lifetime.
    _power_pin: P,
    power_on_delay: Duration,
    timeout: Duration,
    ctx: RenderContext,
    initialized: bool,
    power_needs_settle: bool,
    deadline: Option<Instant>,
}

impl<'a, D, R, P> PointingModeDisplayProcessor<'a, D, R, P>
where
    D: DisplayDriver,
    R: DisplayRenderer<D::Color>,
    P: OutputPin,
{
    pub fn new(
        keymap: &'a KeyMap<'a>,
        display: D,
        renderer: R,
        mut power_pin: P,
        power_pin_low_active: bool,
        _cut_ext_vcc: bool,
        timeout: Duration,
        power_on_delay: Duration,
    ) -> Self {
        set_pin_active(&mut power_pin, power_pin_low_active, true);
        Self {
            keymap,
            display,
            renderer,
            _power_pin: power_pin,
            power_on_delay,
            timeout,
            ctx: RenderContext::default(),
            initialized: false,
            power_needs_settle: true,
            deadline: None,
        }
    }

    async fn wake_and_render(&mut self, event: PointingProcessorEvent) {
        if event.device_id != 0 && event.device_id != 2 {
            return;
        }
        self.ctx.pointing_device_id = event.device_id;
        self.ctx.pointing_mode = Some(event.mode);
        let mode_slot = if event.device_id == 2 { 1 } else { 0 };
        self.ctx.pointing_modes[mode_slot] = Some(event.mode);
        self.ctx.layer = self.keymap.active_layer();
        let leds = crate::keyboard::current_led_indicator();
        self.ctx.num_lock = leds.num_lock();
        self.ctx.caps_lock = leds.caps_lock();
        self.ctx.scroll_lock = leds.scroll_lock();
        #[cfg(feature = "_ble")]
        {
            self.ctx.battery = crate::event::BatteryStatusEvent(
                crate::input_device::battery::current_battery_status(),
            );
        }

        if self.power_needs_settle {
            Timer::after(self.power_on_delay).await;
            self.power_needs_settle = false;
        }

        if !self.initialized {
            // Cold starts can overlap BLE radio bring-up. Waited power settling
            // happens above; failed I2C attempts are bounded and reported by the
            // driver instead of being mistaken for success.
            for attempt in 0..3 {
                if self.display.init().await {
                    self.initialized = true;
                    break;
                }
                if attempt < 2 {
                    Timer::after_millis(200).await;
                }
            }
            if !self.initialized {
                self.fail_keep_power();
                return;
            }
        }

        if !self.display.set_display_on(true).await {
            self.initialized = false;
            self.fail_keep_power();
            return;
        }
        self.renderer.render(&self.ctx, &mut self.display);
        if !self.display.flush().await {
            self.initialized = false;
            self.fail_keep_power();
            return;
        }
        POINTING_MODE_DISPLAY_AWAKE.store(true, Ordering::Release);
        self.deadline = Some(Instant::now() + self.timeout);
    }

    fn fail_keep_power(&mut self) {
        POINTING_MODE_DISPLAY_AWAKE.store(false, Ordering::Release);
        // Keep ExtVCC asserted after an I2C failure. On battery power the OLED
        // can need longer than the first boot attempt to become responsive;
        // the next mode-key event retries initialization on an already-stable
        // rail instead of starting another power cycle.
        self.power_needs_settle = false;
        self.initialized = false;
        self.deadline = None;
    }

    async fn turn_off(&mut self) {
        POINTING_MODE_DISPLAY_AWAKE.store(false, Ordering::Release);
        let _ = self.display.set_display_on(false).await;
        // Only blank the controller; the OLED supply remains asserted for the
        // entire lifetime of this processor, including I2C failures.
        // The next mode key only sends DisplayOn + framebuffer data, with no
        // one-second power-settle delay and no visible reinitialization flash.
        self.deadline = None;
    }
}

impl<D, R, P> Runnable for PointingModeDisplayProcessor<'_, D, R, P>
where
    D: DisplayDriver,
    R: DisplayRenderer<D::Color>,
    P: OutputPin,
{
    async fn run(&mut self) -> ! {
        // Subscribe first so the persisted-mode event cannot be missed while
        // the battery-cold-start display initialization is in progress.
        let mut subscriber = PointingProcessorEvent::subscriber();
        self.wake_and_render(PointingProcessorEvent {
            device_id: 0,
            mode: PointingMode::Cursor(Default::default()),
        })
        .await;

        loop {
            if let Some(deadline) = self.deadline {
                match select(Timer::at(deadline), subscriber.next_event()).await {
                    Either::First(_) => self.turn_off().await,
                    Either::Second(event) => self.wake_and_render(event).await,
                }
            } else {
                if self.initialized {
                    let event = subscriber.next_event().await;
                    self.wake_and_render(event).await;
                } else {
                    // BLE cold starts can leave the first I2C attempts without an
                    // acknowledgement. Keep ExtVCC on and retry after it settles,
                    // even if the user does not press a mode key again.
                    match select(Timer::after_secs(3), subscriber.next_event()).await {
                        Either::First(_) => {
                            self.wake_and_render(PointingProcessorEvent {
                                device_id: self.ctx.pointing_device_id,
                                mode: self.ctx.pointing_mode.unwrap_or_default(),
                            })
                            .await;
                        }
                        Either::Second(event) => self.wake_and_render(event).await,
                    }
                }
            }
        }
    }
}

fn set_pin_active<P: OutputPin>(pin: &mut P, low_active: bool, active: bool) {
    if active ^ low_active {
        let _ = pin.set_high();
    } else {
        let _ = pin.set_low();
    }
}
