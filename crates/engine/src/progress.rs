//! `progress/Progress.h/.cpp`: overall progress estimation reported to the front-end.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Start = 0,
    SplitMultimaterial = 1,
    Slicing = 2,
    Parts = 3,
    InsetSkin = 4,
    Support = 5,
    Export = 6,
    Finish = 7,
}

const TIMES: [f64; 8] = [0.0, 50.0, 5.269, 1.533, 71.811, 51.009, 154.62, 0.1];

fn calc_overall_progress(stage: Stage, stage_progress: f64) -> f64 {
    let stage_progress = stage_progress.clamp(0.0, 1.0);
    let s = stage as usize;
    let accumulated: f64 = TIMES[..s].iter().sum();
    let total: f64 = TIMES.iter().sum();
    (accumulated + stage_progress * TIMES[s]) / total
}

/// `Progress::messageProgress`.
pub fn message_progress(stage: Stage, progress_in_stage: i64, progress_in_stage_max: i64) {
    if !crate::application::has_communication() || progress_in_stage_max == 0 {
        return;
    }
    let percentage = calc_overall_progress(stage, progress_in_stage as f64 / progress_in_stage_max as f64);
    crate::application::communication().send_progress(percentage);
}
