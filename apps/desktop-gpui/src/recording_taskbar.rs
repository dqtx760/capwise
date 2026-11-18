use anyhow::Context;
use cap_recording::screen_capture::ScreenCaptureTarget;
use scap_targets::{
    Display,
    bounds::{LogicalBounds, LogicalPosition, LogicalSize},
};
use windows_sys::Win32::{
    Foundation::RECT,
    Graphics::Gdi::{GetMonitorInfoW, MONITORINFO},
};

pub(crate) fn crop_target(
    target: &ScreenCaptureTarget,
) -> anyhow::Result<Option<ScreenCaptureTarget>> {
    let (screen, selection) = match target {
        ScreenCaptureTarget::Display { id } => (id, None),
        ScreenCaptureTarget::Area { screen, bounds } => (screen, Some(*bounds)),
        ScreenCaptureTarget::Window { .. } | ScreenCaptureTarget::CameraOnly => return Ok(None),
    };
    let unavailable = crate::i18n::tr::dialog::capture::area_unavailable;
    let display = Display::from_id(screen).with_context(unavailable)?;
    let logical_size = display.logical_size().with_context(unavailable)?;
    let empty = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        rcMonitor: empty,
        rcWork: empty,
        dwFlags: 0,
    };
    anyhow::ensure!(
        unsafe { GetMonitorInfoW(display.raw_handle().inner().0, &mut info) } != 0,
        "{}",
        unavailable()
    );
    let full = LogicalBounds::new(LogicalPosition::new(0., 0.), logical_size);
    let original = selection.unwrap_or(full);
    let bounds = crop_bounds(info.rcMonitor, info.rcWork, logical_size, original)
        .with_context(unavailable)?;
    if bounds.position() == original.position() && bounds.size() == original.size() {
        return Ok(None);
    }

    Ok(Some(ScreenCaptureTarget::Area {
        screen: screen.clone(),
        bounds,
    }))
}

fn crop_bounds(
    screen: RECT,
    work: RECT,
    logical: LogicalSize,
    selection: LogicalBounds,
) -> Option<LogicalBounds> {
    let width = f64::from(screen.right) - f64::from(screen.left);
    let height = f64::from(screen.bottom) - f64::from(screen.top);
    let x_scale = logical.width() / width;
    let y_scale = logical.height() / height;
    let x = |edge: i32| (f64::from(edge) - f64::from(screen.left)) * x_scale;
    let y = |edge: i32| (f64::from(edge) - f64::from(screen.top)) * y_scale;

    let position = selection.position();
    let size = selection.size();
    let left = position.x().max(x(work.left.max(screen.left)));
    let top = position.y().max(y(work.top.max(screen.top)));
    let right = (position.x() + size.width()).min(x(work.right.min(screen.right)));
    let bottom = (position.y() + size.height()).min(y(work.bottom.min(screen.bottom)));

    let width = right - left;
    let height = bottom - top;
    (x_scale > 0. && y_scale > 0. && width >= x_scale * 2. && height >= y_scale * 2.).then(|| {
        LogicalBounds::new(
            LogicalPosition::new(left, top),
            LogicalSize::new(width, height),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> RECT {
        RECT {
            left,
            top,
            right,
            bottom,
        }
    }

    #[test]
    fn bottom_taskbar_on_scaled_secondary_monitor_only_shortens_capture() {
        let screen = rect(-1920, 0, 0, 1080);
        let work = rect(-1920, 0, 0, 1020);
        let logical = LogicalSize::new(1536., 864.);
        let full = LogicalBounds::new(LogicalPosition::new(0., 0.), logical);
        let result = crop_bounds(screen, work, logical, full).unwrap();

        assert_eq!(result.position(), LogicalPosition::new(0., 0.));
        assert_eq!(result.size(), LogicalSize::new(1536., 816.));
    }

    #[test]
    fn side_taskbar_clips_a_selected_area_or_rejects_an_empty_intersection() {
        let screen = rect(0, -800, 1280, 0);
        let work = rect(40, -800, 1280, 0);
        let logical = LogicalSize::new(1280., 800.);
        let selection = LogicalBounds::new(
            LogicalPosition::new(25., 100.),
            LogicalSize::new(100., 100.),
        );
        let result = crop_bounds(screen, work, logical, selection).unwrap();

        assert_eq!(result.position(), LogicalPosition::new(40., 100.));
        assert_eq!(result.size(), LogicalSize::new(85., 100.));

        let taskbar = LogicalBounds::new(LogicalPosition::new(0., 0.), LogicalSize::new(39., 100.));
        assert!(crop_bounds(screen, work, logical, taskbar).is_none());
    }
}
