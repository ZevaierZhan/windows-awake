#[derive(Clone, Debug)]
pub struct Config {
    pub idle_seconds: u32,
    pub power_only: bool,
    pub paused: bool,
    pub run_for_seconds: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExternalPower {
    Connected,
    Battery,
    Unknown,
}

impl ExternalPower {
    pub fn from_ac_status(status: u8) -> Self {
        match status {
            1 => Self::Connected,
            0 => Self::Battery,
            _ => Self::Unknown,
        }
    }
}

pub fn should_keep_awake(enabled: bool, interactive: bool, supply: ExternalPower) -> bool {
    enabled && interactive && supply == ExternalPower::Connected
}

#[derive(Debug, Default)]
pub enum AutoDisplay {
    #[default]
    Pending,
    HardwareOff,
    Fallback(String),
    UserAwake,
}
impl AutoDisplay {
    pub fn should_probe(&self, active: bool, power_only: bool) -> bool {
        active && !power_only && matches!(self, Self::Pending)
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            idle_seconds: 30,
            power_only: false,
            paused: false,
            run_for_seconds: None,
        }
    }
}

impl Config {
    pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Option<Self>, String> {
        let mut config = Self::default();
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--idle-seconds" => {
                    config.idle_seconds = args
                        .next()
                        .ok_or("缺少空闲秒数")?
                        .parse()
                        .map_err(|_| "空闲秒数必须是整数")?;
                    if !(5..=3600).contains(&config.idle_seconds) {
                        return Err("空闲秒数范围为 5～3600".into());
                    }
                }
                "--power-only" => config.power_only = true,
                "--paused" => config.paused = true,
                "--run-for-seconds" => {
                    let seconds = args
                        .next()
                        .ok_or("缺少运行秒数")?
                        .parse()
                        .map_err(|_| "运行秒数必须是整数")?;
                    if seconds == 0 {
                        return Err("运行秒数必须大于零".into());
                    }
                    config.run_for_seconds = Some(seconds);
                }
                "--help" | "-h" => return Ok(None),
                _ => return Err(format!("未知参数：{arg}")),
            }
        }
        Ok(Some(config))
    }
}

/// DWORD timestamps wrap every 49.7 days; never subtract them as signed numbers.
pub fn idle_millis(now: u32, last_input: u32) -> u32 {
    now.wrapping_sub(last_input)
}

pub fn should_pulse(
    enabled: bool,
    power_only: bool,
    interactive: bool,
    idle: u32,
    threshold: u32,
) -> bool {
    enabled && !power_only && interactive && idle >= threshold.saturating_mul(1000)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn automatic_display_selection_does_not_retry_or_reblank() {
        let mut mode = AutoDisplay::default();
        assert!(!mode.should_probe(false, false));
        assert!(!mode.should_probe(true, true));
        assert!(mode.should_probe(true, false));
        for selected in [
            AutoDisplay::HardwareOff,
            AutoDisplay::Fallback("unsupported".into()),
            AutoDisplay::UserAwake,
        ] {
            mode = selected;
            assert!(!mode.should_probe(true, false));
        }
        // A new active period (reconnect/resume) triggers a fresh detection.
        mode = AutoDisplay::Pending;
        assert!(mode.should_probe(true, false));
    }
    #[test]
    fn external_power_required_for_both_modes() {
        let sequence = [1, 0, 255, 2, 1];
        let active: Vec<bool> = sequence
            .into_iter()
            .map(|status| should_keep_awake(true, true, ExternalPower::from_ac_status(status)))
            .collect();
        assert_eq!(active, [true, false, false, false, true]);
        for supply in [
            ExternalPower::Connected,
            ExternalPower::Battery,
            ExternalPower::Unknown,
        ] {
            assert!(!should_keep_awake(false, true, supply));
            assert!(!should_keep_awake(true, false, supply));
            let enabled = should_keep_awake(true, true, supply);
            assert_eq!(
                should_pulse(enabled, false, true, 60000, 30),
                supply == ExternalPower::Connected
            );
            assert!(!should_pulse(enabled, true, true, 60000, 30));
        }
    }
    #[test]
    fn handles_tick_wrap() {
        assert_eq!(idle_millis(20, u32::MAX - 9), 30);
    }
    #[test]
    fn respects_activity_pause_and_session() {
        assert!(!should_pulse(true, false, true, 29999, 30));
        assert!(should_pulse(true, false, true, 30000, 30));
        assert!(!should_pulse(false, false, true, 60000, 30));
        assert!(!should_pulse(true, true, true, 60000, 30));
        assert!(!should_pulse(true, false, false, 60000, 30));
    }
    #[test]
    fn validates_options() {
        let parse = |s: &[&str]| Config::parse(s.iter().map(|s| s.to_string()));
        assert_eq!(parse(&[]).unwrap().unwrap().idle_seconds, 30);
        assert!(parse(&["--idle-seconds", "4"]).is_err());
        assert!(parse(&["--idle-seconds"]).is_err());
        assert!(parse(&["--run-for-seconds", "0"]).is_err());
        assert!(parse(&["--unknown"]).is_err());
        assert!(parse(&["--help"]).unwrap().is_none());
        let c = parse(&["--idle-seconds", "60", "--power-only", "--paused"])
            .unwrap()
            .unwrap();
        assert_eq!(c.idle_seconds, 60);
        assert!(c.power_only && c.paused);
    }
}
