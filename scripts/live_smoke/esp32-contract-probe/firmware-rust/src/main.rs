use std::io::Write as _;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context};
use embedded_graphics::mono_font::ascii::{FONT_10X20, FONT_6X10};
use embedded_graphics::mono_font::MonoTextStyle;
use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{PrimitiveStyle, Rectangle};
use embedded_graphics::text::{Baseline, Text};
use embedded_svc::http::client::Client as HttpClient;
use embedded_svc::http::Method;
use embedded_svc::io::Write as _;
use embedded_svc::wifi::{AuthMethod, ClientConfiguration, Configuration as WifiConfiguration};
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::delay::FreeRtos;
use esp_idf_svc::hal::gpio::{AnyInputPin, Output, PinDriver, Pins};
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::hal::spi::{self, SpiDeviceDriver, SpiDriver, SpiDriverConfig, SPI2};
use esp_idf_svc::hal::units::*;
use esp_idf_svc::http::client::{Configuration as HttpConfiguration, EspHttpConnection, Response};
use esp_idf_svc::log::EspLogger;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::sntp::EspSntp;
use esp_idf_svc::sys;
use esp_idf_svc::wifi::{BlockingWifi, EspWifi};
use mipidsi::interface::SpiInterface;
use mipidsi::models::ST7789;
use mipidsi::Builder;
use serde_json::json;
use serde_json::Value;

const WIFI_SSID: Option<&str> = option_env!("WIFI_SSID");
const WIFI_PASS: Option<&str> = option_env!("WIFI_PASS");
const OPENAI_API_KEY: Option<&str> = option_env!("OPENAI_API_KEY");
const OPENAI_MODEL: &str = match option_env!("OPENAI_MODEL") {
    Some(value) => value,
    None => "gpt-4.1-mini",
};
const OPENAI_BASE_URL: &str = match option_env!("OPENAI_BASE_URL") {
    Some(value) => value,
    None => "https://api.openai.com",
};
const LCD_WIDTH: u16 = 170;
const LCD_HEIGHT: u16 = 320;
const LCD_OFFSET_X: u16 = 35;
const LCD_OFFSET_Y: u16 = 0;

type LcdSpi<'d> = SpiDeviceDriver<'d, SpiDriver<'d>>;
type LcdDc<'d> = PinDriver<'d, Output>;
type LcdReset<'d> = PinDriver<'d, Output>;
type LcdInterface<'d> = SpiInterface<'static, LcdSpi<'d>, LcdDc<'d>>;
type LcdPanel<'d> = mipidsi::Display<LcdInterface<'d>, ST7789, LcdReset<'d>>;

struct StatusDisplay<'d> {
    panel: LcdPanel<'d>,
    _backlight: PinDriver<'d, Output>,
}

fn main() {
    sys::link_patches();
    EspLogger::initialize_default();

    if let Err(error) = run_probe() {
        emit_marker(
            "MKT:RUST_STACK:FAIL",
            &[("error", &json_quote(&error.to_string()))],
        );
        emit_marker(
            "MKT:SINGLE_NODE:FAIL",
            &[("error", &json_quote(&error.to_string()))],
        );
        eprintln!("{error:#}");
        std::process::exit(1);
    }
}

fn run_probe() -> anyhow::Result<()> {
    let wifi_ssid = required_build_input(WIFI_SSID, "WIFI_SSID")?;
    let wifi_pass = required_build_input(WIFI_PASS, "WIFI_PASS")?;
    let openai_api_key = required_build_input(OPENAI_API_KEY, "OPENAI_API_KEY")?;

    emit_marker(
        "MKT:BOOT:OK",
        &[(
            "heap_free",
            &unsafe { sys::esp_get_free_heap_size() }.to_string(),
        )],
    );

    let peripherals = Peripherals::take().context("failed to take peripherals")?;
    let modem = peripherals.modem;
    let spi2 = peripherals.spi2;
    let pins = peripherals.pins;
    let sys_loop = EspSystemEventLoop::take().context("failed to take system event loop")?;
    let nvs = EspDefaultNvsPartition::take().context("failed to take default nvs partition")?;

    let mut status_display = match StatusDisplay::try_new(spi2, pins) {
        Ok(mut display) => {
            emit_marker("MKT:DISPLAY:OK", &[("mode", "\"st7789-status\"")]);
            let _ = display.show_stage(
                "boot",
                "phase 0 contract probe",
                Some("bringing up board"),
                Rgb565::new(0, 32, 72),
            );
            Some(display)
        }
        Err(error) => {
            emit_marker(
                "MKT:DISPLAY:FAIL",
                &[(
                    "error",
                    &json_quote(&short_display_line(&error.to_string(), 40)),
                )],
            );
            eprintln!("display init failed: {error:#}");
            None
        }
    };

    let mut wifi = BlockingWifi::wrap(
        EspWifi::new(modem, sys_loop.clone(), Some(nvs)).context("failed to create esp wifi")?,
        sys_loop,
    )
    .context("failed to wrap blocking wifi")?;

    if let Some(display) = status_display.as_mut() {
        let _ = display.show_stage("wifi", "connecting", Some(wifi_ssid), Rgb565::new(0, 0, 80));
    }
    connect_wifi(&mut wifi, wifi_ssid, wifi_pass, status_display.as_mut())?;

    let _sntp = EspSntp::new_default().context("failed to start SNTP")?;
    if let Some(display) = status_display.as_mut() {
        let _ = display.show_stage("time", "syncing sntp", None, Rgb565::new(0, 48, 48));
    }
    wait_for_time_sync(status_display.as_mut())?;

    let stream_started = Instant::now();
    if let Some(display) = status_display.as_mut() {
        let _ = display.show_stage(
            "tls",
            "opening https",
            Some("streaming openai"),
            Rgb565::new(56, 28, 0),
        );
    }
    let stream_events = match run_openai_stream(openai_api_key) {
        Ok(events) => events,
        Err(error) => {
            if let Some(display) = status_display.as_mut() {
                let _ = display.show_stage(
                    "fail",
                    "stream error",
                    Some(&short_display_line(&error.to_string(), 28)),
                    Rgb565::new(88, 0, 0),
                );
            }
            return Err(error);
        }
    };
    let elapsed_ms = stream_started.elapsed().as_millis();

    emit_marker(
        "MKT:RUST_STACK:OK",
        &[
            ("lane", "\"std-blocking\""),
            ("stream_events", &stream_events.to_string()),
            ("elapsed_ms", &elapsed_ms.to_string()),
            (
                "heap_free",
                &unsafe { sys::esp_get_free_heap_size() }.to_string(),
            ),
        ],
    );
    emit_marker(
        "MKT:SINGLE_NODE:PASS",
        &[
            ("lane", "\"std-blocking\""),
            ("elapsed_ms", &elapsed_ms.to_string()),
        ],
    );

    if let Some(display) = status_display.as_mut() {
        let detail = format!("{stream_events} events {elapsed_ms} ms");
        let _ = display.show_stage(
            "pass",
            "single node ok",
            Some(&detail),
            Rgb565::new(0, 72, 0),
        );
    }

    thread::sleep(Duration::from_secs(2));
    Ok(())
}

fn connect_wifi(
    wifi: &mut BlockingWifi<EspWifi<'static>>,
    wifi_ssid: &str,
    wifi_pass: &str,
    status_display: Option<&mut StatusDisplay<'_>>,
) -> anyhow::Result<()> {
    let configuration = WifiConfiguration::Client(ClientConfiguration {
        ssid: wifi_ssid
            .try_into()
            .map_err(|_| anyhow!("wifi ssid is too long"))?,
        bssid: None,
        auth_method: AuthMethod::WPA2Personal,
        password: wifi_pass
            .try_into()
            .map_err(|_| anyhow!("wifi password is too long"))?,
        channel: None,
        ..Default::default()
    });

    wifi.set_configuration(&configuration)
        .context("failed to set wifi configuration")?;
    wifi.start().context("failed to start wifi")?;
    wifi.connect().context("failed to connect wifi")?;
    wifi.wait_netif_up().context("wifi netif did not come up")?;

    let ip_info = wifi
        .wifi()
        .sta_netif()
        .get_ip_info()
        .context("failed to read station ip info")?;
    emit_marker(
        "MKT:WIFI:OK",
        &[("ip", &json_quote(&ip_info.ip.to_string()))],
    );
    if let Some(display) = status_display {
        let detail = format!("ip {}", ip_info.ip);
        let _ = display.show_stage("wifi", "connected", Some(&detail), Rgb565::new(0, 72, 0));
    }
    Ok(())
}

fn wait_for_time_sync(status_display: Option<&mut StatusDisplay<'_>>) -> anyhow::Result<()> {
    let mut status_display = status_display;
    for _ in 0..30 {
        let now = unix_time_secs();
        if now > 1_700_000_000 {
            emit_marker("MKT:TIME:OK", &[("unix", &now.to_string())]);
            if let Some(display) = status_display.as_mut() {
                let detail = format!("unix {now}");
                let _ = display.show_stage(
                    "time",
                    "clock synced",
                    Some(&detail),
                    Rgb565::new(0, 56, 40),
                );
            }
            return Ok(());
        }
        thread::sleep(Duration::from_secs(1));
    }

    bail!("timed out waiting for SNTP time sync")
}

fn run_openai_stream(openai_api_key: &str) -> anyhow::Result<usize> {
    let body = json!({
        "model": OPENAI_MODEL,
        "input": [
            {
                "type": "message",
                "role": "user",
                "content": "Write twelve short lowercase words about embedded streaming, separated by spaces."
            }
        ],
        "max_output_tokens": 64,
        "stream": true
    });
    let payload = serde_json::to_vec(&body).context("failed to serialize request body")?;
    let auth_header = format!("Bearer {openai_api_key}");
    let content_length = payload.len().to_string();
    let url = format!("{OPENAI_BASE_URL}/v1/responses");
    let headers = [
        ("content-type", "application/json"),
        ("authorization", auth_header.as_str()),
        ("content-length", content_length.as_str()),
    ];

    let http_config = HttpConfiguration {
        timeout: Some(Duration::from_secs(60)),
        buffer_size: Some(2048),
        buffer_size_tx: Some(2048),
        crt_bundle_attach: Some(sys::esp_crt_bundle_attach),
        keep_alive_enable: true,
        ..Default::default()
    };
    let mut client = HttpClient::wrap(
        EspHttpConnection::new(&http_config).context("failed to create HTTPS client")?,
    );

    let mut request = client
        .request(Method::Post, &url, &headers)
        .context("failed to create request")?;
    request
        .write_all(&payload)
        .context("failed to write request payload")?;
    request.flush().context("failed to flush request payload")?;

    let mut response = request.submit().context("failed to submit request")?;
    let status = response.status();
    let content_type = response
        .header("content-type")
        .unwrap_or("unknown")
        .to_string();

    if !(200..=299).contains(&status) {
        let body = read_response_body(&mut response)?;
        bail!("provider status {status}: {body}");
    }

    emit_marker(
        "MKT:TLS:OK",
        &[
            ("status", &status.to_string()),
            ("content_type", &json_quote(&content_type)),
        ],
    );
    emit_marker(
        "MKT:PROVIDER:STREAM_START",
        &[("model", &json_quote(OPENAI_MODEL))],
    );

    let stream_events = read_sse_stream(&mut response)?;
    emit_marker(
        "MKT:PROVIDER:STREAM_DONE",
        &[("events", &stream_events.to_string())],
    );

    Ok(stream_events)
}

fn read_sse_stream(response: &mut Response<&mut EspHttpConnection>) -> anyhow::Result<usize> {
    let mut buf = [0_u8; 512];
    let mut pending = String::new();
    let mut saw_done = false;
    let mut saw_output_event = false;
    let mut event_seq = 0_usize;

    'outer: loop {
        let read = response
            .read(&mut buf)
            .map_err(|error| anyhow!("failed to read response body: {error}"))?;
        if read == 0 {
            break;
        }

        pending.push_str(&String::from_utf8_lossy(&buf[..read]));

        while let Some(newline_index) = pending.find('\n') {
            let line = pending[..newline_index].trim_end_matches('\r').to_string();
            pending.replace_range(..=newline_index, "");

            let should_stop =
                handle_sse_line(&line, &mut saw_done, &mut saw_output_event, &mut event_seq)?;
            if should_stop {
                break 'outer;
            }
        }
    }

    if !saw_done && !pending.trim().is_empty() {
        handle_sse_line(
            pending.trim_end_matches('\r'),
            &mut saw_done,
            &mut saw_output_event,
            &mut event_seq,
        )?;
    }

    if !saw_output_event {
        bail!("stream completed without an output event");
    }
    if !saw_done {
        bail!("stream completed without a done marker");
    }

    Ok(event_seq)
}

fn handle_sse_line(
    line: &str,
    saw_done: &mut bool,
    saw_output_event: &mut bool,
    event_seq: &mut usize,
) -> anyhow::Result<bool> {
    let Some(data) = line.strip_prefix("data:") else {
        return Ok(false);
    };
    let payload = data.trim_start();

    if payload.is_empty() {
        return Ok(false);
    }
    if payload == "[DONE]" {
        *saw_done = true;
        return Ok(true);
    }

    let event: Value = serde_json::from_str(payload)
        .with_context(|| format!("failed to parse SSE payload: {payload}"))?;
    if let Some(error) = event.get("error") {
        bail!("provider stream error: {error}");
    }

    let event_type = event
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    if event_type.ends_with(".delta")
        || (!*saw_output_event && response_completed_contains_text(&event))
    {
        *saw_output_event = true;
        *event_seq += 1;
        emit_marker(
            "MKT:PROVIDER:CHUNK",
            &[
                ("seq", &event_seq.to_string()),
                ("type", &json_quote(event_type)),
                ("bytes", &payload.len().to_string()),
            ],
        );
    }

    if matches!(event_type, "response.done" | "response.completed") {
        *saw_done = true;
        return Ok(true);
    }

    Ok(false)
}

fn response_completed_contains_text(event: &Value) -> bool {
    event
        .get("response")
        .and_then(|response| response.get("output"))
        .and_then(Value::as_array)
        .is_some_and(|items| {
            items.iter().any(|item| {
                item.get("type").and_then(Value::as_str) == Some("message")
                    && item
                        .get("content")
                        .and_then(Value::as_array)
                        .is_some_and(|content| {
                            content.iter().any(|block| {
                                block.get("type").and_then(Value::as_str) == Some("output_text")
                            })
                        })
            })
        })
}

fn read_response_body(response: &mut Response<&mut EspHttpConnection>) -> anyhow::Result<String> {
    let mut buf = [0_u8; 512];
    let mut body = String::new();
    loop {
        let read = response
            .read(&mut buf)
            .map_err(|error| anyhow!("failed to read error body: {error}"))?;
        if read == 0 {
            break;
        }
        body.push_str(&String::from_utf8_lossy(&buf[..read]));
    }
    Ok(body)
}

fn unix_time_secs() -> u64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_secs(),
        Err(_) => 0,
    }
}

fn emit_marker(marker: &str, fields: &[(&str, &str)]) {
    if fields.is_empty() {
        println!("{marker}");
    } else {
        let suffix = fields
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect::<Vec<_>>()
            .join(" ");
        println!("{marker} {suffix}");
    }
    let _ = std::io::stdout().flush();
}

fn json_quote(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"<encoding-error>\"".to_string())
}

impl<'d> StatusDisplay<'d> {
    fn try_new(spi: SPI2<'d>, pins: Pins) -> anyhow::Result<Self> {
        let dc = PinDriver::output(pins.gpio11).context("failed to configure lcd dc")?;
        let rst = PinDriver::output(pins.gpio9).context("failed to configure lcd rst")?;
        let mut backlight =
            PinDriver::output(pins.gpio14).context("failed to configure lcd backlight")?;
        let spi_device = SpiDeviceDriver::new_single(
            spi,
            pins.gpio10,
            pins.gpio13,
            None::<AnyInputPin>,
            Some(pins.gpio12),
            &SpiDriverConfig::new(),
            &spi::config::Config::new().baudrate(40.MHz().into()),
        )
        .context("failed to configure lcd spi")?;

        let buffer = Box::leak(Box::new([0_u8; 1024]));
        let interface = SpiInterface::new(spi_device, dc, buffer);
        let mut delay = FreeRtos;
        let mut panel = Builder::new(ST7789, interface)
            .reset_pin(rst)
            .display_size(LCD_WIDTH, LCD_HEIGHT)
            .display_offset(LCD_OFFSET_X, LCD_OFFSET_Y)
            .init(&mut delay)
            .map_err(|error| anyhow!("failed to init st7789 panel: {error:?}"))?;

        panel
            .clear(Rgb565::BLACK)
            .map_err(|error| anyhow!("failed to clear lcd: {error:?}"))?;
        backlight
            .set_low()
            .map_err(|error| anyhow!("failed to enable lcd backlight: {error:?}"))?;

        Ok(Self {
            panel,
            _backlight: backlight,
        })
    }

    fn show_stage(
        &mut self,
        stage: &str,
        headline: &str,
        detail: Option<&str>,
        background: Rgb565,
    ) -> anyhow::Result<()> {
        Rectangle::new(
            Point::zero(),
            Size::new(LCD_WIDTH.into(), LCD_HEIGHT.into()),
        )
        .into_styled(PrimitiveStyle::with_fill(background))
        .draw(&mut self.panel)
        .map_err(|error| anyhow!("failed to paint lcd background: {error:?}"))?;

        let header_style = MonoTextStyle::new(&FONT_10X20, Rgb565::WHITE);
        let body_style = MonoTextStyle::new(&FONT_6X10, Rgb565::WHITE);

        Text::with_baseline(
            &stage.to_uppercase(),
            Point::new(8, 14),
            header_style,
            Baseline::Top,
        )
        .draw(&mut self.panel)
        .map_err(|error| anyhow!("failed to draw lcd stage: {error:?}"))?;

        Text::with_baseline(
            &short_display_line(headline, 24),
            Point::new(8, 48),
            body_style,
            Baseline::Top,
        )
        .draw(&mut self.panel)
        .map_err(|error| anyhow!("failed to draw lcd headline: {error:?}"))?;

        if let Some(detail) = detail {
            Text::with_baseline(
                &short_display_line(detail, 28),
                Point::new(8, 66),
                body_style,
                Baseline::Top,
            )
            .draw(&mut self.panel)
            .map_err(|error| anyhow!("failed to draw lcd detail: {error:?}"))?;
        }

        Ok(())
    }
}

fn short_display_line(input: &str, max_chars: usize) -> String {
    let mut out = input.replace('\n', " ");
    if out.chars().count() <= max_chars {
        return out;
    }
    out = out
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    out.push('…');
    out
}

fn required_build_input<'a>(value: Option<&'a str>, name: &str) -> anyhow::Result<&'a str> {
    match value {
        Some(value) if !value.is_empty() => Ok(value),
        _ => bail!("missing required build input {name}"),
    }
}
