# rmknumouse nice!nano 腳位備存

更新日期：2026-09-20  
來源：目前 `/home/rmk/keyboard/rmknumouse/keyboard.toml` 與 `vial.json`

## 目前使用中的腳位

| 功能 | nice!nano / nRF52840 GPIO | 方向與設定 | 備註 |
|---|---|---|---|
| 實體鍵 1 | `P1_13` | active-low input、pull-up | Matrix `(0,0)`，預設 `A` |
| 實體鍵 2 | `P1_04` | active-low input、pull-up | Matrix `(0,1)`，預設 `B` |
| 實體鍵 3 | `P0_05` | active-low input、pull-up | Matrix `(0,2)`，預設 `C` |
| Joystick J1 X | `P0_31` / AIN7 | ADC input | bias `29130` |
| Joystick J1 Y | `P0_29` / AIN5 | ADC input | bias `29365`；與預留 J2 X 衝突 |
| Joystick J1 電源 | `P1_15` | switched output | 每次取樣時短暫供電 |
| Encoder A | `P0_06` | input | 使用外部上拉；與預留 OLED SCL 衝突 |
| Encoder B | `P0_08` | input | 使用外部上拉 |
| PMW3610 SDIO | `P0_17` | half-duplex data | `MOSI` 與 `MISO` 使用同一腳 |
| PMW3610 SCLK | `P0_20` | output | bitbang SPI clock |
| PMW3610 CS | `P0_22` | output | chip select |
| PMW3610 Motion | `P0_24` | input | motion interrupt，用於低功耗喚醒 |
| EXT_VCC enable | `P0_13` | output high | 供應 PMW3610 與 encoder 外部上拉；不可改成 `_` 或高阻態 |

## 邏輯 Matrix 與 Vial

目前 matrix 為 2 rows × 4 cols。

| 座標 | 用途 | 實體 GPIO |
|---|---|---|
| `(0,0)` | 實體鍵 1 | `P1_13` |
| `(0,1)` | 實體鍵 2 | `P1_04` |
| `(0,2)` | 實體鍵 3 | `P0_05` |
| `(0,3)` | 未使用 | 無 |
| `(1,0)` | User WASD：Up | 虛擬位置，無 GPIO |
| `(1,1)` | User WASD：Left | 虛擬位置，無 GPIO |
| `(1,2)` | User WASD：Right | 虛擬位置，無 GPIO |
| `(1,3)` | User WASD：Down | 虛擬位置，無 GPIO |

Encoder 0 另外提供兩個 Vial 槽位：

- `0,0`：逆時針，預設 `AudioVolDown`
- `0,1`：順時針，預設 `AudioVolUp`

## nice!nano 板載與預留腳位

| 功能 | GPIO | 目前狀態 |
|---|---|---|
| 板載藍色 LED | `P0_15` | 韌體未設定 |
| 電池電壓 ADC | `P0_04` / AIN2 | nice!nano 板載使用；目前 `keyboard.toml` 未明確設定 |
| EXT_VCC enable | `P0_13` | 使用中，開機後輸出高電位 |
| OLED SDA | `P0_26` | 預留、停用 |
| OLED SCL | `P0_06` | 預留、停用；目前已由 Encoder A 使用 |
| Joystick J2 X | `P0_29` | 預留；目前已由 J1 Y 使用 |
| Joystick J2 Y | `P0_02` | 預留、停用 |
| Joystick J2 電源 | `P1_00` | 預留，必須保持關閉 |

## 目前電源設定

```toml
[chip.nrf52840]
dcdc_reg0 = false
dcdc_reg1 = true
```

這裡記錄的是目前實際編譯設定。日後針對 nice!nano 最低功耗調校時，可再依實際電源電路確認 `dcdc_reg0` 是否應啟用。

EXT_VCC 目前設定：

```toml
[[output]]
pin = "P0_13"
initial_state_active = true
low_active = false
```

`P0_13` 原本放在 direct-pin matrix 時，是靠 active-low input 自動提供的 pull-up 維持外設供電。精簡 matrix 後必須保留上面的明確 output 設定，否則 PMW3610 會斷電，encoder 的外部上拉也會失去電源。

## 目前已釋放的舊按鍵腳位

以下腳位不再屬於三鍵 matrix；是否能改作其他用途，仍需先對照 nice!nano 板載功能與 PCB 走線：

- `P1_06`
- `P0_10`
- `P0_09`
- `P1_10`
- `P1_11`

`P0_13` 雖然不再是按鍵，但仍是必要的 EXT_VCC enable，不是可自由使用的腳位。
