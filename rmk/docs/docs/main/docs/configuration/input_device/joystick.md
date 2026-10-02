# Joysticks

A joystick is an analog input device that can be used for mouse control and other functions. Currently, only NRF series chips are supported.

::: warning

1. You need to use a debug probe to find your parameters now.
2. Only Nrf is supported now.

:::

TODO:

- [ ] a more intuitive way to configure the joystick
- [ ] more functions besides mouse

## `toml` configuration

```toml
[[input_device.joystick]]
name = "default"
# id = 0
pin_x = "P0_31"
pin_y = "P0_29"
pin_z = "_"
transform = [[80, 0], [0, 80]]
bias = [29130, 29365]
resolution = 6
# func = "mouse | n-direction key" # TODO: only mouse is supported now
```

### Parameters:

- `name`: Unique name for the joystick. If you have multiple joysticks, they need different names
- `id`: Optional device id used to match this joystick with its `JoystickProcessor`. If omitted, ids are assigned sequentially starting from 0
- `pin_x`: Pin for X-axis
- `pin_y`: Pin for Y-axis
- `pin_z`: Pin for Z-axis
- `transform`: Transformation matrix for the joystick
- `bias`: Bias value for each axis
- `resolution`: Resolution for each axis

::: note
`_` indicates that the axis does not exist. The TOML mouse joystick requires X and Y; use `_` for an absent Z axis. The bias vector and square transform matrix must match the configured axis count.

For example: `pin_x = "_"` `pin_y = "P0_29"` `pin_z = "P0_30"` is not allowed
:::

::: tip
The transform might be not very intuitive, please read the document below for more information.
:::

#### How it works

1. Device reads values from each axis
2. Adds the `bias` value to each axis to make the value close to 0 when the joystick is released
3. About the `transform` matrix:
   1. New x-axis value = (axis_x + bias[0]) / transform[0][0] + (axis_y + bias[1]) / transform[0][1] + (axis_z + bias[2]) / transform[0][2]
   2. New y-axis value = (axis_x + bias[0]) / transform[1][0] + (axis_y + bias[1]) / transform[1][1] + (axis_z + bias[2]) / transform[1][2]
   3. New z-axis value = (axis_x + bias[0]) / transform[2][0] + (axis_y + bias[1]) / transform[2][1] + (axis_z + bias[2]) / transform[2][2]

   If `transform[new_axis][old_axis]` is 0, that old axis value is ignored.

   Since the value range read by the ADC device is usually much larger than the mouse report range of -128~127 (values outside it are clamped), `transform` is designed as a divisor.

4. Each axis value is adjusted to the largest integer multiple of `resolution` that is less than its original value to reduce noise from ADC device readings.

#### How to find configuration for your hardware quickly

1. First set `bias` to 0, `resolution` to 1, and `transform` to `[[1, 0, 0], [0, 1, 0], [0, 0, 1]]` (matrix dimension depends on the number of axes)

2. Find the optimal `bias` value:
   - Use a debug probe to find the output `JoystickProcessor::generate_report: record = [axis_x, axis_y, axis_z]` in debug information
   - Observe these values to find the `bias` value that makes each axis closest to 0 when the joystick is released

3. If the mouse moves too fast, gradually increase the `transform` value until you find the right sensitivity

4. If the mouse jitters, gradually increase the `resolution` value until the jitter disappears

### Power-managed sampling (nRF52)

The TOML joystick uses an active/idle sampling schedule. These fields may be added to the joystick entry above:

```toml
# power_pin = "P0_10" # optional, requires suitable supply-control wiring
polling_rate_hz = 50
idle_polling_rate_hz = 10
sample_settle_us = 20
boot_settle_ms = 2
deadzone = 0
```

The values shown are defaults. Both rates must be nonzero, with the idle rate no greater than the active rate. All joysticks sharing the SAADC must use the same rates and settling times. Bias and deadzone remain per joystick.

With `power_pin`, RMK raises the pin before sampling, waits an additional `boot_settle_ms` on first startup and `sample_settle_us` before each conversion, then lowers the pin after sampling. Cancellation and a 5 ms ADC timeout also release the supply. The settling time must be chosen for the actual joystick and circuit. Supply switching requires appropriate wiring; use a transistor or load switch when the joystick cannot be supplied safely by the GPIO.

Without `power_pin`, the supply is not switched, but the new sampling schedule still applies. After all joystick X/Y axes remain within their deadzones for 1.2 seconds, sampling switches to the idle rate. An out-of-deadzone sample restores the active rate. Detection is limited by the idle sampling interval.

`deadzone` is in axis counts after bias and before transform/resolution, in `0..=32767`. Values inside it become zero; values outside it have the deadzone magnitude subtracted. Fixed bias is retained; there is no automatic center calibration.

Existing TOML files still parse, but their sampling behavior changes: the former 350 ms light-sleep interval is replaced by the default 100 ms idle interval and center-based idle detection. Repeated stationary HID reports are still emitted by this sampling change. Report filtering is a separate change.

When battery measurement shares the SAADC, battery reporting is scheduled every 30 seconds. Its additional conversion runs with joystick power off; it uses the shared ADC rather than a second ADC device.

## `rust` configuration

Because the `joystick` and `battery` use the same ADC peripheral, they actually use the same `NrfAdc` `input_device`.

The example below uses the original `NrfAdc::new` schedule without `.with_power_management(...)`. If the `light_sleep` is not `None`, the `NrfAdc` will enter light sleep mode when no event is generated after 1200ms, and the polling interval will be changed to the value assigned.

```rust
use embassy_nrf::saadc::{self, Input as _};
use embassy_time::Duration;

let saadc_config = saadc::Config::default();
let adc = saadc::Saadc::new(p.SAADC, Irqs, saadc_config,
    [
        saadc::ChannelConfig::single_ended(saadc::VddhDiv5Input.degrade_saadc()),
        saadc::ChannelConfig::single_ended(p.P0_31.degrade_saadc()),
        saadc::ChannelConfig::single_ended(p.P0_29.degrade_saadc())
    ],
);
adc.calibrate().await;
let mut adc_dev = NrfAdc::new(
    adc,
    [AnalogEventType::Battery, AnalogEventType::Joystick(2)],
    [0, 0], // device id per event; unused for battery events
    Duration::from_millis(20), // polling interval
    Some(Duration::from_millis(350)), // light sleep interval
);
let mut batt_proc = BatteryProcessor::new(1, 5);
let mut joy_proc = JoystickProcessor::new(0, [[80, 0], [0, 80]], [29130, 29365], 6, &keymap);
...
run_all!(matrix, adc_dev, joy_proc, batt_proc)
...
```
