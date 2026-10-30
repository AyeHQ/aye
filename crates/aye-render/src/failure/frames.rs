//! A failure's frames: where it is shown ([`origin`], [`pointed`]), its
//! frames as L3 lists them ([`frame_items`]), and the innermost frame's
//! locals ([`locals`]).

use aye_events::{Failure, Frame, FramePath, Local};

use crate::words::plural;

/// A failure's frames as L3 lists them: the project's each, and each run
/// of other frames as one entry per package.
pub(crate) enum FrameItem<'a> {
    Project(&'a Frame),
    /// `count` frames in a row outside the project, in `package`.
    Others {
        count: usize,
        package: String,
    },
}

pub(crate) fn frame_items(frames: &[Frame]) -> Vec<FrameItem<'_>> {
    let mut items: Vec<FrameItem<'_>> = Vec::new();
    for frame in frames {
        if frame.path.is_project() {
            items.push(FrameItem::Project(frame));
            continue;
        }
        let package = package_of(frame.path.as_str());
        match items.last_mut() {
            Some(FrameItem::Others {
                count,
                package: last,
            }) if *last == package => *count += 1,
            _ => items.push(FrameItem::Others { count: 1, package }),
        }
    }
    items
}

/// `12 frames in requests`.
pub(crate) fn others(count: usize, package: &str) -> String {
    format!("{} in {package}", plural(count, "frame", "frames"))
}

/// The package a frame outside the project belongs to: the first part
/// after `site-packages` (or `dist-packages`), the standard library's
/// module after `pythonX.Y` (or `Lib`), else the file's directory.
pub(crate) fn package_of(path: &str) -> String {
    let path = path.replace('\\', "/");
    if path.starts_with('<') {
        return path.trim_matches(['<', '>']).to_owned();
    }
    let parts: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
    let after = parts.iter().rposition(|part| {
        matches!(*part, "site-packages" | "dist-packages" | "Lib")
            || (part.starts_with("python") && part[6..].starts_with(|c: char| c.is_ascii_digit()))
    });
    let part = match after {
        Some(at) if at + 1 < parts.len() => parts[at + 1],
        _ if parts.len() >= 2 => parts[parts.len() - 2],
        _ => parts.last().copied().unwrap_or_default(),
    };
    part.strip_suffix(".py").unwrap_or(part).to_owned()
}

/// The frame a failure is shown at ([`origin`]), with its source.
pub(crate) fn pointed(failure: &Failure) -> Option<&Frame> {
    let (path, line) = origin(failure)?;
    failure
        .frames
        .iter()
        .rev()
        .find(|frame| frame.path == *path && frame.line == line)
}

/// The local variables a failure's innermost frame had.
pub(crate) fn locals(failure: &Failure) -> &[Local] {
    failure
        .frames
        .iter()
        .rev()
        .find(|frame| !frame.locals.is_empty())
        .map_or(&[], |frame| frame.locals.as_slice())
}

/// Where to look: where the failure was raised when that is in the project,
/// else the innermost project frame, else wherever it was raised.
pub(crate) fn origin(failure: &Failure) -> Option<(&FramePath, u32)> {
    let raised = failure
        .location
        .as_ref()
        .map(|location| (&location.path, location.line))
        .or_else(|| failure.frames.last().map(|frame| (&frame.path, frame.line)));
    if let Some(raised) = raised.filter(|(path, _)| path.is_project()) {
        return Some(raised);
    }
    failure
        .frames
        .iter()
        .rev()
        .find(|frame| frame.path.is_project())
        .map(|frame| (&frame.path, frame.line))
        .or(raised)
}

/// `path:line in function: source`.
pub(crate) fn frame_line(frame: &Frame) -> String {
    let mut line = format!("{}:{}", frame.path, frame.line);
    if let Some(function) = &frame.function {
        line.push_str(" in ");
        line.push_str(function);
    }
    if let Some(source) = &frame.source {
        line.push_str(": ");
        line.push_str(source);
    }
    line
}
