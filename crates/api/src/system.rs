// SPDX-License-Identifier: AGPL-3.0-or-later

//! The load of the machine the API runs on (CPU, memory, load average),
//! sampled every few seconds and sent to operators on the live WebSocket
//! (`system` message). See `docs/web.md#status-bar`.

use std::time::Duration;

use serde::Serialize;
use sysinfo::{MINIMUM_CPU_UPDATE_INTERVAL, System};
use tokio::sync::watch;
use utoipa::ToSchema;

/// How often the machine is read.
pub const INTERVAL: Duration = Duration::from_secs(3);

/// What the status bar shows of the machine.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, ToSchema)]
pub struct SystemLoad {
    /// CPU in use, 0 to 100, over all cores.
    pub cpu_percent: f32,
    /// Memory in use (not counting what the system can give back), 0 to 100.
    pub mem_percent: f32,
    /// The load average over 1, 5 and 15 minutes.
    pub load_avg: [f32; 3],
    /// Logical cores, to read the load average against.
    pub cores: u32,
}

/// The latest reading, `None` until the first one is made.
pub type SystemWatch = watch::Receiver<Option<SystemLoad>>;

/// `part` out of `total`, as a percentage kept between 0 and 100.
fn percent(part: f64, total: f64) -> f32 {
    if total <= 0.0 || !part.is_finite() {
        return 0.0;
    }
    (part / total * 100.0).clamp(0.0, 100.0) as f32
}

/// Makes a reading from raw figures.
pub fn reading(
    cpu: f32,
    used_memory: u64,
    total_memory: u64,
    load: [f64; 3],
    cores: usize,
) -> SystemLoad {
    let finite = |value: f64| if value.is_finite() { value as f32 } else { 0.0 };
    SystemLoad {
        cpu_percent: percent(f64::from(cpu), 100.0),
        mem_percent: percent(used_memory as f64, total_memory as f64),
        load_avg: [finite(load[0]), finite(load[1]), finite(load[2])],
        cores: u32::try_from(cores).unwrap_or(u32::MAX),
    }
}

/// Reads the machine every [`INTERVAL`] and publishes it.
pub async fn run(sender: watch::Sender<Option<SystemLoad>>) {
    let mut system = match System::new() {
        Ok(system) => system,
        Err(error) => {
            // No reading: the status bar goes without its system part.
            tracing::warn!(%error, "cannot read the machine's load");
            return;
        }
    };
    // The CPU in use is the change between two readings.
    system.refresh_cpu_usage();
    tokio::time::sleep(MINIMUM_CPU_UPDATE_INTERVAL).await;
    let mut tick = tokio::time::interval(INTERVAL);
    loop {
        tick.tick().await;
        system.refresh_cpu_usage();
        system.refresh_memory();
        let load =
            System::load_average().map_or([0.0; 3], |load| [load.one, load.five, load.fifteen]);
        let value = reading(
            system.global_cpu_usage(),
            system
                .total_memory()
                .saturating_sub(system.available_memory()),
            system.total_memory(),
            load,
            system.cpus().len(),
        );
        if sender.send(Some(value)).is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentages_stay_between_0_and_100() {
        let value = reading(250.0, 3, 4, [0.5, 1.0, 1.5], 4);
        assert_eq!(value.cpu_percent, 100.0);
        assert_eq!(value.mem_percent, 75.0);
        assert_eq!(value.load_avg, [0.5, 1.0, 1.5]);
        assert_eq!(value.cores, 4);
        assert_eq!(reading(-3.0, 9, 4, [0.0; 3], 1).cpu_percent, 0.0);
        assert_eq!(reading(0.0, 9, 4, [0.0; 3], 1).mem_percent, 100.0);
    }

    #[test]
    fn a_machine_without_memory_figures_reads_zero() {
        let value = reading(f32::NAN, 5, 0, [f64::NAN, 0.0, f64::INFINITY], 0);
        assert_eq!(value.cpu_percent, 0.0);
        assert_eq!(value.mem_percent, 0.0);
        assert_eq!(value.load_avg, [0.0; 3]);
    }

    #[tokio::test]
    async fn publishes_a_first_reading() {
        let (sender, mut receiver) = watch::channel(None);
        let task = tokio::spawn(run(sender));
        tokio::time::timeout(Duration::from_secs(10), receiver.changed())
            .await
            .expect("a reading within 10 s")
            .expect("the sampler is running");
        let value = (*receiver.borrow()).expect("a reading");
        assert!((0.0..=100.0).contains(&value.cpu_percent));
        assert!((0.0..=100.0).contains(&value.mem_percent));
        task.abort();
    }
}
