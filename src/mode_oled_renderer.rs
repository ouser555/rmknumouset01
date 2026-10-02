use embedded_graphics::mono_font::{ascii::FONT_5X8, MonoTextStyle};
use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::*;
use embedded_graphics::text::Text;
use rmk::display::{DisplayRenderer, RenderContext};
use rmk::input_device::pointing::PointingMode;
use rmk::types::battery::BatteryStatus;

#[derive(Default)]
pub struct ModeOledRenderer;

fn mode_name(mode: Option<PointingMode>) -> &'static str {
    match mode {
        Some(PointingMode::UserWasd) => "UWASD",
        Some(PointingMode::Wasd) => "WASD",
        Some(PointingMode::Cursor(_)) => "MOUSE",
        Some(PointingMode::HiResScroll(_)) => "SCROL",
        Some(PointingMode::Joystick) => "JOY",
        Some(PointingMode::SpaceMouse) => "SPACE",
        Some(_) => "OTHER",
        None => "START",
    }
}

fn draw_line<D: DrawTarget<Color = BinaryColor>>(
    display: &mut D,
    style: MonoTextStyle<'_, BinaryColor>,
    row: i32,
    text: &str,
) {
    Text::new(text, Point::new(0, row * 8 + 7), style)
        .draw(display)
        .ok();
}

impl DisplayRenderer<BinaryColor> for ModeOledRenderer {
    fn render<D: DrawTarget<Color = BinaryColor>>(&mut self, ctx: &RenderContext, display: &mut D) {
        display.clear(BinaryColor::Off).ok();
        let style = MonoTextStyle::new(&FONT_5X8, BinaryColor::On);

        let mut battery_line = heapless::String::<6>::new();
        match ctx.battery.0 {
            BatteryStatus::Available {
                level: Some(level), ..
            } => {
                core::fmt::write(&mut battery_line, format_args!("B{level:>3}%")).ok();
            }
            _ => {
                battery_line.push_str("B --%").ok();
            }
        }

        let mut layer_line = heapless::String::<6>::new();
        match ctx.layer {
            0 => layer_line.push_str("BASE").ok(),
            1 => layer_line.push_str("SPACE").ok(),
            layer => core::fmt::write(&mut layer_line, format_args!("L{layer}")).ok(),
        };

        draw_line(display, style, 0, &battery_line);
        draw_line(display, style, 1, "-----");
        draw_line(display, style, 2, "LAYER");
        draw_line(display, style, 3, &layer_line);
        draw_line(display, style, 4, "-----");
        draw_line(display, style, 5, "STATS");
        draw_line(
            display,
            style,
            6,
            if ctx.num_lock { "NUM:@" } else { "NUM:_" },
        );
        draw_line(
            display,
            style,
            7,
            if ctx.caps_lock { "CAP:@" } else { "CAP:_" },
        );
        draw_line(
            display,
            style,
            8,
            if ctx.scroll_lock { "SCR:@" } else { "SCR:_" },
        );
        draw_line(display, style, 9, "-----");
        draw_line(display, style, 10, "LEFT");
        draw_line(display, style, 11, mode_name(ctx.pointing_modes[0]));
        draw_line(display, style, 12, "RIGHT");
        draw_line(display, style, 13, mode_name(ctx.pointing_modes[1]));
        draw_line(display, style, 14, "-----");
        draw_line(
            display,
            style,
            15,
            if matches!(ctx.pointing_modes[0], Some(PointingMode::SpaceMouse)) {
                "SPACE"
            } else {
                "NUMOU"
            },
        );
    }
}
