//! Battery level from the GPIO4 divider. The board's charger exposes no status pin and there is
//! no VBUS sense, so "charging" is inferred from voltage, USB host presence and the voltage trend.

use esp_idf_hal::adc::attenuation::DB_12;
use esp_idf_hal::adc::oneshot::config::{AdcChannelConfig, Calibration};
use esp_idf_hal::adc::oneshot::{AdcChannelDriver, AdcDriver};
use esp_idf_hal::adc::{ADC1, ADCCH3, ADCU1};
use esp_idf_hal::gpio::Gpio4;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

const SAMPLE_PERIOD: Duration = Duration::from_secs(1);
const READS_PER_SAMPLE: u32 = 8;
const SMOOTHING_SAMPLES: usize = 10;
// One trend point every 10s over a ~60s window: slow enough to ignore load-induced ripple.
const TREND_PERIOD: Duration = Duration::from_secs(10);
const TREND_POINTS: usize = 7;
const TREND_RISE_MV: u32 = 15;
// With no cell attached the charger output floats well above a full LiPo's 4.2V.
const EXTERNAL_POWER_MV: u32 = 4300;

// Loaded LiPo voltage to state of charge; linear in between.
const CURVE: [(u32, u8); 11] = [
    (3300, 0),
    (3500, 5),
    (3600, 10),
    (3700, 25),
    (3750, 40),
    (3800, 55),
    (3850, 65),
    (3900, 75),
    (4000, 85),
    (4100, 95),
    (4150, 100),
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct BatteryStatus {
    pub percent: u8,
    pub charging: bool,
}

type Channel = AdcChannelDriver<'static, ADCCH3<ADCU1>, AdcDriver<'static, ADCU1>>;

pub struct Battery {
    channel: Channel,
    samples: VecDeque<u32>,
    trend: VecDeque<u32>,
    last_sample: Option<Instant>,
    last_trend: Option<Instant>,
    status: Option<BatteryStatus>,
}

impl Battery {
    pub fn new(adc1: ADC1<'static>, pin: Gpio4<'static>) -> anyhow::Result<Self> {
        let config = AdcChannelConfig { attenuation: DB_12, calibration: Calibration::Curve, ..Default::default() };
        let channel = AdcChannelDriver::new(AdcDriver::new(adc1)?, pin, &config)?;
        Ok(Self {
            channel,
            samples: VecDeque::with_capacity(SMOOTHING_SAMPLES),
            trend: VecDeque::with_capacity(TREND_POINTS),
            last_sample: None,
            last_trend: None,
            status: None,
        })
    }

    /// Samples at most once per `SAMPLE_PERIOD`; returns the status only when it changed.
    pub fn poll(&mut self) -> Option<BatteryStatus> {
        if self.last_sample.is_some_and(|t| t.elapsed() < SAMPLE_PERIOD) {
            return None;
        }
        self.last_sample = Some(Instant::now());

        let mv = match self.read_battery_mv() {
            Ok(mv) => mv,
            Err(e) => {
                log::warn!("battery: ADC read failed: {e:?}");
                return None;
            }
        };
        push_bounded(&mut self.samples, mv, SMOOTHING_SAMPLES);
        let avg = self.samples.iter().sum::<u32>() / self.samples.len() as u32;

        if self.last_trend.map_or(true, |t| t.elapsed() >= TREND_PERIOD) {
            self.last_trend = Some(Instant::now());
            push_bounded(&mut self.trend, avg, TREND_POINTS);
        }

        let next = self.classify(avg);
        log::debug!("battery: {mv} mV (avg {avg} mV) -> {next:?}");
        if self.status == Some(next) {
            return None;
        }
        log::info!("battery: {avg} mV -> {}% charging={}", next.percent, next.charging);
        self.status = Some(next);
        self.status
    }

    fn classify(&self, avg_mv: u32) -> BatteryStatus {
        if avg_mv >= EXTERNAL_POWER_MV {
            return BatteryStatus { percent: 100, charging: true };
        }
        let charging = usb_host_connected() || self.voltage_rising();
        let measured = percent_from_mv(avg_mv);
        // Only move with the charge direction, so ADC and load noise can't make the number flicker.
        let percent = match self.status {
            Some(prev) if prev.charging == charging && charging => measured.max(prev.percent),
            Some(prev) if prev.charging == charging => measured.min(prev.percent),
            _ => measured,
        };
        BatteryStatus { percent, charging }
    }

    fn voltage_rising(&self) -> bool {
        match (self.trend.front(), self.trend.back()) {
            (Some(&oldest), Some(&newest)) if self.trend.len() == TREND_POINTS => newest >= oldest + TREND_RISE_MV,
            _ => false,
        }
    }

    fn read_battery_mv(&mut self) -> anyhow::Result<u32> {
        let mut total = 0u32;
        for _ in 0..READS_PER_SAMPLE {
            total += self.channel.read()? as u32;
        }
        // 100k/100k divider in front of the ADC pin.
        Ok(total / READS_PER_SAMPLE * 2)
    }
}

fn push_bounded(buf: &mut VecDeque<u32>, value: u32, cap: usize) {
    if buf.len() == cap {
        buf.pop_front();
    }
    buf.push_back(value);
}

/// Only catches a USB host (computer), not a wall charger; those rely on `voltage_rising`.
fn usb_host_connected() -> bool {
    unsafe { esp_idf_svc::sys::usb_serial_jtag_is_connected() }
}

fn percent_from_mv(mv: u32) -> u8 {
    let (first_mv, first_pct) = CURVE[0];
    if mv <= first_mv {
        return first_pct;
    }
    for pair in CURVE.windows(2) {
        let ((lo_mv, lo_pct), (hi_mv, hi_pct)) = (pair[0], pair[1]);
        if mv <= hi_mv {
            let span = (hi_pct - lo_pct) as u32;
            return lo_pct + ((mv - lo_mv) * span / (hi_mv - lo_mv)) as u8;
        }
    }
    100
}
