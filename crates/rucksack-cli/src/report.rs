use crate::output::Output;
use anyhow::Result;
use chrono::Local;
use rucksack_core::network::{read_interface_counters, InterfaceCounters};
use rucksack_core::{
    AppPaths, MobileDataEstimate, MobileDataUsage, SessionEndKind, SessionReport, SessionState,
};

#[derive(Debug)]
pub struct MobileDataBaseline {
    pub counters: Option<InterfaceCounters>,
    pub error: Option<String>,
}

pub fn read_mobile_data_baseline(interface: &str, output: &Output) -> MobileDataBaseline {
    match read_interface_counters(interface) {
        Ok(counters) => MobileDataBaseline {
            counters: Some(counters),
            error: None,
        },
        Err(error) => {
            let message =
                format!("Could not start mobile-data accounting on {interface}: {error:#}");
            output.warn(&message);
            MobileDataBaseline {
                counters: None,
                error: Some(message),
            }
        }
    }
}

pub fn sample_mobile_data(session: &mut SessionState, final_sample: bool) {
    let Some(start) = session.mobile_data_start.as_ref() else {
        session.mobile_data_finalized = false;
        if session.mobile_data_error.is_none() {
            session.mobile_data_error =
                Some("No interface-counter baseline was recorded".to_owned());
        }
        return;
    };
    match read_interface_counters(&start.interface) {
        Ok(counters) => {
            session.mobile_data_end = Some(counters);
            session.mobile_data_finalized = final_sample;
            session.mobile_data_error = None;
        }
        Err(error) => {
            session.mobile_data_finalized = false;
            session.mobile_data_error = Some(format!(
                "Could not read {} interface counters at session end: {error:#}",
                start.interface
            ));
        }
    }
}

pub fn archive_session(
    paths: &AppPaths,
    session: &SessionState,
    end_kind: SessionEndKind,
) -> Result<SessionReport> {
    let report = SessionReport::from_session(session, end_kind)?;
    let persisted = report.save(paths)?;
    if persisted.session_id != session.id {
        anyhow::bail!(
            "Refusing to finalize session {} because the persisted report belongs to session {}",
            session.id,
            persisted.session_id
        );
    }
    Ok(persisted)
}

pub fn run(output: &Output, paths: &AppPaths) -> Result<()> {
    let report = SessionReport::load(paths)?.ok_or_else(|| {
        anyhow::anyhow!(
            "No completed Rucksack session report is available. Run `rucksack pack` to start one."
        )
    })?;
    show(output, &report)
}

pub fn show(output: &Output, report: &SessionReport) -> Result<()> {
    if !output.json() {
        output.title("Last session report");
    }
    show_body(output, report)
}

pub fn show_body(output: &Output, report: &SessionReport) -> Result<()> {
    if output.json() {
        return output.emit_json(report);
    }

    output.pass(format!(
        "{} · {}",
        report.agent.display_name(),
        report.project_dir.display()
    ));
    output.plain(format!(
        "Duration {} · ended {}",
        format_duration(report.duration_seconds),
        report
            .released_at
            .with_timezone(&Local)
            .format("%Y-%m-%d %H:%M")
    ));
    output.plain(format!(
        "Ended by {}: {}",
        end_kind_name(report.end_kind),
        report.release_reason
    ));
    match (report.start_battery_percent, report.end_battery_percent) {
        (Some(start), Some(end)) => output.plain(format!("Battery {start}% → {end}%")),
        (_, Some(end)) => output.plain(format!("Battery at end {end}%")),
        _ => output.warn("Battery usage unavailable"),
    }
    if let Some(ssid) = report.expected_hotspot_ssid.as_deref() {
        output.plain(format!("Hotspot {ssid}"));
    }
    if let Some(interface) = report.route_interface.as_deref() {
        let gateway = report
            .route_gateway
            .as_deref()
            .map(|value| format!(" via {value}"))
            .unwrap_or_default();
        output.plain(format!("Commute route {interface}{gateway}"));
    }
    render_mobile_data(output, &report.mobile_data);
    Ok(())
}

fn render_mobile_data(output: &Output, estimate: &MobileDataEstimate) {
    match estimate {
        MobileDataEstimate::Available { usage } => {
            output.plain(format_mobile_data(usage));
            output.plain(format!(
                "Aggregate traffic on {}; not agent-only usage or carrier billing.",
                usage.interface
            ));
        }
        MobileDataEstimate::Partial { usage, reason } => {
            output.plain(format_mobile_data(usage));
            output.warn(format!("Mobile-data estimate is partial: {reason}"));
            output.plain(format!(
                "Aggregate traffic on {}; not agent-only usage or carrier billing.",
                usage.interface
            ));
        }
        MobileDataEstimate::Unavailable { reason } => {
            output.warn(format!("Estimated mobile data unavailable: {reason}"));
        }
    }
}

fn format_mobile_data(usage: &MobileDataUsage) -> String {
    format!(
        "Estimated mobile data {} total · {} downloaded · {} uploaded",
        format_bytes(usage.total_bytes),
        format_bytes(usage.received_bytes),
        format_bytes(usage.sent_bytes)
    )
}

fn end_kind_name(kind: SessionEndKind) -> &'static str {
    match kind {
        SessionEndKind::Unpack => "unpack",
        SessionEndKind::Automatic => "automatic release",
        SessionEndKind::Recovery => "recovery",
    }
}

fn format_duration(seconds: u64) -> String {
    let hours = seconds / 3_600;
    let minutes = (seconds % 3_600) / 60;
    let seconds = seconds % 60;
    if hours > 0 {
        format!("{hours}h {minutes}m {seconds}s")
    } else if minutes > 0 {
        format!("{minutes}m {seconds}s")
    } else {
        format!("{seconds}s")
    }
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0usize;
    while value >= 1_000.0 && unit < UNITS.len() - 1 {
        value /= 1_000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_decimal_mobile_data_units() {
        assert_eq!(format_bytes(999), "999 B");
        assert_eq!(format_bytes(1_500), "1.5 KB");
        assert_eq!(format_bytes(2_500_000), "2.5 MB");
    }

    #[test]
    fn formats_session_duration() {
        assert_eq!(format_duration(59), "59s");
        assert_eq!(format_duration(125), "2m 5s");
        assert_eq!(format_duration(3_725), "1h 2m 5s");
    }
}
