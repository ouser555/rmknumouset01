use rmk::core_traits::Runnable;
use rmk::embassy_time::Timer;
use rmk::event::{publish_event, ActionEvent, PointingProcessorEvent};
use rmk::input_device::pointing::{HiResScrollConfig, PointingMode, ScrollConfig};
use rmk::keymap::KeyMap;
use rmk::macros::processor;
use rmk::processor::Processor;
use rmk::types::action::Action;

const MODE_KEYCODES: [u8; 2] = [7, 8];
const SPACEMOUSE_KEYCODE: u8 = 9;
const DEVICE_IDS: [u8; 2] = [0, 2];
const SPACEMOUSE_LAYER: u8 = 1;
const PACKED_MARKER: u8 = 0x80;

#[derive(Clone, Copy)]
enum SavedPointingMode {
    Cursor = 0,
    HiResScroll = 2,
    Joystick = 3,
    Wasd = 4,
    UserWasd = 5,
}

impl SavedPointingMode {
    fn from_byte(value: u8) -> Self {
        match value {
            1 | 2 => Self::HiResScroll,
            3 => Self::Joystick,
            4 => Self::Wasd,
            5 => Self::UserWasd,
            _ => Self::Cursor,
        }
    }

    fn next(self) -> Self {
        match self {
            Self::UserWasd => Self::Wasd,
            Self::Wasd => Self::Cursor,
            Self::Cursor => Self::HiResScroll,
            Self::HiResScroll => Self::Joystick,
            Self::Joystick => Self::UserWasd,
        }
    }

    fn pointing_mode(self) -> PointingMode {
        match self {
            Self::Cursor => PointingMode::Cursor(Default::default()),
            Self::HiResScroll => PointingMode::HiResScroll(HiResScrollConfig {
                scroll: ScrollConfig {
                    divisor_x: 5,
                    divisor_y: 5,
                    ..ScrollConfig::default()
                },
                units_per_step: 1,
            }),
            Self::Joystick => PointingMode::Joystick,
            Self::Wasd => PointingMode::Wasd,
            Self::UserWasd => PointingMode::UserWasd,
        }
    }
}

#[processor(subscribe = [ActionEvent])]
#[rmk::macros::runnable_generated]
pub struct PointingModeController<'a> {
    keymap: &'a KeyMap<'a>,
    modes: [SavedPointingMode; 2],
    restored: bool,
    spacemouse_mode: bool,
}

impl<'a> PointingModeController<'a> {
    pub fn new(keymap: &'a KeyMap<'a>) -> Self {
        Self {
            keymap,
            modes: [SavedPointingMode::Cursor; 2],
            restored: false,
            spacemouse_mode: false,
        }
    }

    async fn restore(&mut self) {
        if self.restored {
            return;
        }
        let saved = rmk::storage::read_pointing_mode().await.unwrap_or(0);
        if saved & PACKED_MARKER != 0 {
            self.modes[0] = SavedPointingMode::from_byte(saved & 0x07);
            self.modes[1] = SavedPointingMode::from_byte((saved >> 3) & 0x07);
        } else {
            self.modes[0] = SavedPointingMode::from_byte(saved);
            self.modes[1] = SavedPointingMode::Cursor;
        }
        self.restored = true;

        // Restore J2 first so the startup screen finishes with both modes known.
        self.publish_saved(1);
        self.publish_saved(0);
    }

    fn publish_saved(&self, index: usize) {
        publish_event(PointingProcessorEvent {
            device_id: DEVICE_IDS[index],
            mode: self.modes[index].pointing_mode(),
        });
    }

    fn publish_spacemouse(&self) {
        // J2 remains the right gamepad stick; J1 becomes LX + hi-res wheel.
        publish_event(PointingProcessorEvent {
            device_id: DEVICE_IDS[1],
            mode: PointingMode::Joystick,
        });
        publish_event(PointingProcessorEvent {
            device_id: DEVICE_IDS[0],
            mode: PointingMode::SpaceMouse,
        });
    }

    async fn save(&self) {
        let packed = PACKED_MARKER | self.modes[0] as u8 | ((self.modes[1] as u8) << 3);
        let _ = rmk::storage::write_pointing_mode(packed).await;
    }

    fn toggle_spacemouse(&mut self) {
        self.spacemouse_mode = !self.spacemouse_mode;
        if self.spacemouse_mode {
            self.keymap.activate_layer(SPACEMOUSE_LAYER);
            self.publish_spacemouse();
        } else {
            self.keymap.deactivate_layer(SPACEMOUSE_LAYER);
            self.publish_saved(1);
            self.publish_saved(0);
        }
    }

    async fn on_action_event(&mut self, event: ActionEvent) {
        if event.action == Action::User(SPACEMOUSE_KEYCODE) {
            if event.keyboard_event.pressed {
                self.restore().await;
                self.toggle_spacemouse();
            }
            return;
        }

        if !event.keyboard_event.pressed {
            return;
        }
        let Some(index) = MODE_KEYCODES
            .iter()
            .position(|keycode| event.action == Action::User(*keycode))
        else {
            return;
        };

        self.restore().await;
        if self.spacemouse_mode {
            // In SpaceMouse mode the axis assignments are fixed. Re-publish the
            // selected side's current mode so either mode key wakes the OLED.
            publish_event(PointingProcessorEvent {
                device_id: DEVICE_IDS[index],
                mode: if index == 0 {
                    PointingMode::SpaceMouse
                } else {
                    PointingMode::Joystick
                },
            });
            return;
        }
        if rmk::display::pointing_mode_display_is_awake() {
            self.modes[index] = self.modes[index].next();
            self.save().await;
        }
        self.publish_saved(index);
    }
}

impl Runnable for PointingModeController<'_> {
    async fn run(&mut self) -> ! {
        Timer::after_millis(10).await;
        self.restore().await;
        self.process_loop().await
    }
}
