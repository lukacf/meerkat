use anyhow::{anyhow, bail, Result};
use embedded_svc::wifi::{AuthMethod as ClientAuthMethod, ClientConfiguration};
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::wifi::{
    BlockingWifi, Configuration, EspWifi,
};
use esp_idf_sys as sys;
use std::net::UdpSocket;
use std::ptr;
use std::thread;
use std::time::Duration;

const PIN_SCLK: i32 = 40;
const PIN_MOSI: i32 = 45;
const PIN_DC: i32 = 41;
const PIN_RST: i32 = 39;
const PIN_CS: i32 = 42;
const PIN_BL: i32 = 48;

const LCD_H_RES: usize = 172;
const LCD_V_RES: usize = 320;
const LCD_PIXEL_CLOCK_HZ: u32 = 12_000_000;
const OFFSET_X: u16 = 34;
const WIFI_SSID: &str = match option_env!("WIFI_SSID") {
    Some(value) => value,
    None => "L&L",
};
const WIFI_PASS: &str = match option_env!("WIFI_PASS") {
    Some(value) => value,
    None => "krokodil12",
};
const MARKER_BROADCAST_ADDR: &str = "255.255.255.255:42424";

fn esp_ok(code: i32, context: &str) -> Result<()> {
    if code == 0 {
        Ok(())
    } else {
        bail!("{context} failed with esp_err={code}");
    }
}

fn delay_ms(ms: u64) {
    thread::sleep(Duration::from_millis(ms));
}

unsafe fn gpio_output(pin: i32) -> Result<()> {
    let mut cfg = sys::gpio_config_t::default();
    cfg.pin_bit_mask = 1u64 << pin;
    cfg.mode = sys::gpio_mode_t_GPIO_MODE_OUTPUT;
    esp_ok(sys::gpio_config(&cfg), "gpio_config")
}

unsafe fn gpio_set(pin: i32, high: bool) -> Result<()> {
    esp_ok(
        sys::gpio_set_level(pin as sys::gpio_num_t, if high { 1 } else { 0 }),
        "gpio_set_level",
    )
}

unsafe fn backlight_on() -> Result<()> {
    gpio_output(PIN_BL)?;
    gpio_set(PIN_BL, true)
}

unsafe fn hardware_reset() -> Result<()> {
    gpio_output(PIN_RST)?;
    gpio_set(PIN_RST, false)?;
    delay_ms(20);
    gpio_set(PIN_RST, true)?;
    delay_ms(20);
    Ok(())
}

unsafe fn make_io() -> Result<sys::esp_lcd_panel_io_handle_t> {
    let mut buscfg = sys::spi_bus_config_t::default();
    buscfg.__bindgen_anon_1.mosi_io_num = PIN_MOSI;
    buscfg.__bindgen_anon_2.miso_io_num = -1;
    buscfg.sclk_io_num = PIN_SCLK;
    buscfg.__bindgen_anon_3.quadwp_io_num = -1;
    buscfg.__bindgen_anon_4.quadhd_io_num = -1;
    buscfg.data4_io_num = -1;
    buscfg.data5_io_num = -1;
    buscfg.data6_io_num = -1;
    buscfg.data7_io_num = -1;
    buscfg.max_transfer_sz = (LCD_H_RES * LCD_V_RES * 2) as i32;

    esp_ok(
        sys::spi_bus_initialize(
            sys::spi_host_device_t_SPI3_HOST,
            &buscfg,
            sys::spi_common_dma_t_SPI_DMA_CH_AUTO,
        ),
        "spi_bus_initialize",
    )?;

    let mut io_cfg = sys::esp_lcd_panel_io_spi_config_t::default();
    io_cfg.cs_gpio_num = PIN_CS;
    io_cfg.dc_gpio_num = PIN_DC;
    io_cfg.spi_mode = 0;
    io_cfg.pclk_hz = LCD_PIXEL_CLOCK_HZ;
    io_cfg.trans_queue_depth = 10;
    io_cfg.lcd_cmd_bits = 8;
    io_cfg.lcd_param_bits = 8;

    let mut io_handle: sys::esp_lcd_panel_io_handle_t = ptr::null_mut();
    esp_ok(
        sys::esp_lcd_new_panel_io_spi(
            sys::spi_host_device_t_SPI3_HOST as sys::esp_lcd_spi_bus_handle_t,
            &io_cfg,
            &mut io_handle,
        ),
        "esp_lcd_new_panel_io_spi",
    )?;
    if io_handle.is_null() {
        bail!("panel io handle is null");
    }
    Ok(io_handle)
}

unsafe fn tx(io: sys::esp_lcd_panel_io_handle_t, cmd: u32, params: &[u8]) -> Result<()> {
    esp_ok(
        sys::esp_lcd_panel_io_tx_param(
            io,
            cmd as i32,
            if params.is_empty() {
                ptr::null()
            } else {
                params.as_ptr().cast()
            },
            params.len(),
        ),
        "esp_lcd_panel_io_tx_param",
    )
}

unsafe fn tx_none(io: sys::esp_lcd_panel_io_handle_t, cmd: u32) -> Result<()> {
    tx(io, cmd, &[])
}

unsafe fn init_panel(io: sys::esp_lcd_panel_io_handle_t) -> Result<()> {
    tx_none(io, sys::LCD_CMD_SLPOUT)?;
    delay_ms(100);

    tx(io, 0x36, &[0x40])?;
    tx(io, 0x3A, &[0x55])?;
    tx(io, 0xB0, &[0x00, 0xE8])?;
    tx(io, 0xB2, &[0x0c, 0x0c, 0x00, 0x33, 0x33])?;
    tx(io, 0xB7, &[0x75])?;
    tx(io, 0xBB, &[0x1A])?;
    tx(io, 0xC0, &[0x80])?;
    tx(io, 0xC2, &[0x01, 0xff])?;
    tx(io, 0xC3, &[0x13])?;
    tx(io, 0xC4, &[0x20])?;
    tx(io, 0xC6, &[0x0F])?;
    tx(io, 0xD0, &[0xA4, 0xA1])?;
    tx(
        io,
        0xE0,
        &[0xD0, 0x0D, 0x14, 0x0D, 0x0D, 0x09, 0x38, 0x44, 0x4E, 0x3A, 0x17, 0x18, 0x2F, 0x30],
    )?;
    tx(
        io,
        0xE1,
        &[0xD0, 0x09, 0x0F, 0x08, 0x07, 0x14, 0x37, 0x44, 0x4D, 0x38, 0x15, 0x16, 0x2C, 0x2E],
    )?;
    tx_none(io, 0x21)?;
    tx_none(io, 0x29)?;
    tx_none(io, 0x2C)?;
    Ok(())
}

unsafe fn fill_color(io: sys::esp_lcd_panel_io_handle_t, color: u16) -> Result<()> {
    let x_start = OFFSET_X;
    let x_end = OFFSET_X + LCD_H_RES as u16 - 1;

    let case = [
        (x_start >> 8) as u8,
        (x_start & 0xFF) as u8,
        (x_end >> 8) as u8,
        (x_end & 0xFF) as u8,
    ];
    tx(io, sys::LCD_CMD_CASET, &case)?;

    let mut line = vec![0u8; LCD_H_RES * 2];
    for px in line.chunks_exact_mut(2) {
        px[0] = (color >> 8) as u8;
        px[1] = (color & 0xFF) as u8;
    }

    for y in 0..LCD_V_RES as u16 {
        let ras = [(y >> 8) as u8, (y & 0xFF) as u8, (y >> 8) as u8, (y & 0xFF) as u8];
        tx(io, sys::LCD_CMD_RASET, &ras)?;
        esp_ok(
            sys::esp_lcd_panel_io_tx_color(
                io,
                sys::LCD_CMD_RAMWR as i32,
                line.as_ptr().cast(),
                line.len(),
            ),
            "esp_lcd_panel_io_tx_color",
        )?;
    }
    Ok(())
}

unsafe fn run() -> Result<()> {
    let peripherals =
        Peripherals::take().map_err(|_| anyhow!("failed to take peripherals"))?;
    let modem = peripherals.modem;
    let sys_loop = EspSystemEventLoop::take().map_err(|e| anyhow!("system event loop: {e}"))?;
    let nvs =
        EspDefaultNvsPartition::take().map_err(|e| anyhow!("default nvs partition: {e}"))?;

    let mut wifi = BlockingWifi::wrap(
        EspWifi::new(modem, sys_loop.clone(), Some(nvs))
            .map_err(|e| anyhow!("esp wifi new: {e}"))?,
        sys_loop,
    )
    .map_err(|e| anyhow!("blocking wifi wrap: {e}"))?;

    let client = ClientConfiguration {
        ssid: WIFI_SSID
            .try_into()
            .map_err(|_| anyhow!("wifi ssid too long"))?,
        password: WIFI_PASS
            .try_into()
            .map_err(|_| anyhow!("wifi password too long"))?,
        auth_method: ClientAuthMethod::WPA2Personal,
        ..Default::default()
    };

    wifi.set_configuration(&Configuration::Client(client))
        .map_err(|e| anyhow!("set wifi configuration: {e}"))?;
    wifi.start().map_err(|e| anyhow!("wifi start: {e}"))?;
    wifi.connect().map_err(|e| anyhow!("wifi connect: {e}"))?;
    wifi.wait_netif_up()
        .map_err(|e| anyhow!("wifi netif up: {e}"))?;

    let ip = wifi
        .wifi()
        .sta_netif()
        .get_ip_info()
        .map_err(|e| anyhow!("get ip info: {e}"))?
        .ip
        .to_string();

    let marker = UdpSocket::bind("0.0.0.0:0").map_err(|e| anyhow!("udp bind: {e}"))?;
    marker
        .set_broadcast(true)
        .map_err(|e| anyhow!("udp set_broadcast: {e}"))?;
    let _ = marker.send_to(format!("WS147:NET:READY ip={ip}").as_bytes(), MARKER_BROADCAST_ADDR);

    backlight_on()?;
    hardware_reset()?;
    let io = make_io()?;
    init_panel(io)?;
    let _ = marker.send_to(b"WS147:DISPLAY:INIT_OK", MARKER_BROADCAST_ADDR);
    fill_color(io, 0xF800)?;
    delay_ms(500);
    fill_color(io, 0x07E0)?;
    let _ = marker.send_to(b"WS147:DISPLAY:FILL_OK", MARKER_BROADCAST_ADDR);

    loop {
        let _ = marker.send_to(
            format!("WS147:HEARTBEAT ip={ip}").as_bytes(),
            MARKER_BROADCAST_ADDR,
        );
        delay_ms(1000);
    }
}

fn main() -> Result<()> {
    sys::link_patches();
    unsafe { run() }.map_err(|e| anyhow!("ws147 net smoke failed: {e}"))
}
