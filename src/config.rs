#[derive(Clone, Debug)]
pub struct Config {
    pub idle_seconds: u32,
    pub grass_delay_seconds: u32,
    pub grass_grow_seconds: u32,
    pub grass_hold_seconds: u32,
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
impl Default for Config {
    fn default() -> Self {
        Self {
            idle_seconds: 30,
            grass_delay_seconds: 30,
            grass_grow_seconds: 120,
            grass_hold_seconds: 30,
            power_only: false,
            paused: false,
            run_for_seconds: None,
        }
    }
}
impl Config {
    pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Option<Self>, String> {
        let mut c = Self::default();
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--idle-seconds"
                | "--grass-delay-seconds"
                | "--grass-grow-seconds"
                | "--grass-hold-seconds" => {
                    let value: u32 = args
                        .next()
                        .ok_or("缺少时间参数")?
                        .parse()
                        .map_err(|_| "时间必须为整数")?;
                    if !(1..=3600).contains(&value) {
                        return Err("时间范围为 1～3600 秒".into());
                    }
                    match arg.as_str() {
                        "--idle-seconds" => c.idle_seconds = value,
                        "--grass-delay-seconds" => c.grass_delay_seconds = value,
                        "--grass-grow-seconds" => c.grass_grow_seconds = value,
                        _ => c.grass_hold_seconds = value,
                    }
                }
                "--power-only" => c.power_only = true,
                "--paused" => c.paused = true,
                "--run-for-seconds" => {
                    let n = args
                        .next()
                        .ok_or("缺少运行秒数")?
                        .parse()
                        .map_err(|_| "运行秒数必须为整数")?;
                    if n == 0 {
                        return Err("运行秒数必须大于零".into());
                    }
                    c.run_for_seconds = Some(n);
                }
                "--help" | "-h" => return Ok(None),
                _ => return Err(format!("未知参数：{arg}")),
            }
        }
        Ok(Some(c))
    }
}
#[cfg(test)]
pub fn idle_millis(now: u32, last: u32) -> u32 {
    now.wrapping_sub(last)
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Standby,
    KeepAlive,
    Growing,
    FullGrass,
    ScreenOff,
}
impl Phase {
    pub fn keeps_awake(self) -> bool {
        self != Self::Standby
    }
}
pub fn phase(c: &Config, idle_ms: u64, eligible: bool) -> (Phase, f32) {
    if !eligible || idle_ms < c.idle_seconds as u64 * 1000 {
        return (Phase::Standby, 0.0);
    }
    if c.power_only {
        return (Phase::KeepAlive, 0.0);
    }
    let elapsed = idle_ms - c.idle_seconds as u64 * 1000;
    let delay = c.grass_delay_seconds as u64 * 1000;
    let grow = c.grass_grow_seconds as u64 * 1000;
    if elapsed < delay {
        (Phase::KeepAlive, 0.0)
    } else if elapsed < delay + grow {
        (Phase::Growing, (elapsed - delay) as f32 / grow as f32)
    } else if elapsed < delay + grow + c.grass_hold_seconds as u64 * 1000 {
        (Phase::FullGrass, 1.0)
    } else {
        (Phase::ScreenOff, 1.0)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn timeline_has_separate_full_grass_hold() {
        let c = Config::default();
        for (ms, p) in [
            (29999, Phase::Standby),
            (30000, Phase::KeepAlive),
            (59999, Phase::KeepAlive),
            (60000, Phase::Growing),
            (180000, Phase::FullGrass),
            (209999, Phase::FullGrass),
            (210000, Phase::ScreenOff),
        ] {
            assert_eq!(phase(&c, ms, true).0, p);
        }
        assert_eq!(phase(&c, 120000, true).1, 0.5);
        assert_eq!(phase(&c, 900000, false).0, Phase::Standby);
        assert_eq!(phase(&c, 0, true).0, Phase::Standby);
    }
    #[test]
    fn exclusions_and_input_reset_every_phase() {
        let c = Config::default();
        for ms in [30000, 120000, 190000, 250000] {
            assert!(phase(&c, ms, true).0.keeps_awake());
            assert_eq!(phase(&c, ms, false).0, Phase::Standby);
            assert_eq!(phase(&c, 0, true).0, Phase::Standby);
        }
        let c = Config {
            power_only: true,
            ..c
        };
        assert_eq!(phase(&c, 999999, true).0, Phase::KeepAlive);
    }
    #[test]
    fn validates_timing_and_tick_wrap() {
        assert_eq!(idle_millis(20, u32::MAX - 9), 30);
        let parse = |s: &[&str]| Config::parse(s.iter().map(|s| s.to_string()));
        assert!(parse(&["--grass-grow-seconds", "0"]).is_err());
        assert!(parse(&["--grass-delay-seconds"]).is_err());
        assert!(parse(&["--help"]).unwrap().is_none());
        assert_eq!(
            parse(&["--idle-seconds", "2", "--grass-grow-seconds", "3"])
                .unwrap()
                .unwrap()
                .grass_grow_seconds,
            3
        );
    }
}
