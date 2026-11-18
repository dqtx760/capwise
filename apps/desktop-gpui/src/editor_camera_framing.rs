use cap_project::{
    Camera, CameraShape, ColorCorrection, CornerStyle, SceneMode, SceneSegment, SplitCameraFraming,
    XY,
};
use gpui::{
    AnyElement, AppContext, Context, Entity, FontWeight, Hsla, InteractiveElement, IntoElement,
    MouseButton, ParentElement, SharedString, StatefulInteractiveElement, Styled, Subscription,
    Window, div, px,
};

use crate::{
    editor_sidebar::SliderKey,
    editor_tabs::{CameraSlider, OpenMenu, SidebarMenu},
    editor_timeline::{self, TrackKind},
    editor_window::EditorWindow,
    i18n::tr,
    recording_defaults::StudioLayout,
    ui,
};

#[derive(Default)]
pub(crate) struct CameraFramingUi {
    pub crop_open: bool,
    dialog: Option<RatioDialog>,
}

struct RatioDialog {
    width: Entity<ui::TextInputState>,
    height: Entity<ui::TextInputState>,
    error: bool,
    _subscriptions: Vec<Subscription>,
}

impl CameraFramingUi {
    pub fn is_open(&self) -> bool {
        self.dialog.is_some()
    }
}

fn parse_ratio(width: &str, height: &str) -> Option<XY<u32>> {
    let x = width.trim().parse::<u32>().ok()?;
    let y = height.trim().parse::<u32>().ok()?;
    ((1..=100).contains(&x)
        && (1..=100).contains(&y)
        && (0.25..=4.0).contains(&(f64::from(x) / f64::from(y))))
    .then_some(XY::new(x, y))
}

const LAYOUT_CHOICES: [(StudioLayout, fn() -> &'static str, &str); 4] = [
    (
        StudioLayout::CameraOnly,
        tr::main::studio_layout::camera_only,
        "icons/crop.svg",
    ),
    (
        StudioLayout::PictureInPicture,
        tr::main::studio_layout::pip,
        "icons/app-window.svg",
    ),
    (
        StudioLayout::ScreenLeft,
        tr::main::studio_layout::screen_left,
        "icons/panel-right.svg",
    ),
    (
        StudioLayout::CameraLeft,
        tr::main::studio_layout::camera_left,
        "icons/panel-left.svg",
    ),
];

const FRAMING_CHOICES: [(SplitCameraFraming, fn() -> &'static str); 3] = [
    (
        SplitCameraFraming::Fill,
        tr::main::studio_layout::framing_fill,
    ),
    (
        SplitCameraFraming::Relaxed,
        tr::main::studio_layout::framing_relaxed,
    ),
    (
        SplitCameraFraming::Fit,
        tr::main::studio_layout::framing_fit,
    ),
];

fn camera_scene_time(time: f64, end: f64) -> f64 {
    time.clamp(0., (end - 1e-9).max(0.))
}

fn camera_scene_at(scenes: &[SceneSegment], time: f64) -> Option<usize> {
    scenes
        .iter()
        .position(|scene| time >= scene.start && time < scene.end)
}

fn camera_scene_ranges(
    selection: Option<&crate::editor_edits::Selection>,
    clips: &[(f64, f64)],
    playhead: f64,
    total_duration: f64,
) -> Vec<(f64, f64)> {
    if !total_duration.is_finite() || total_duration <= 0.0 {
        return Vec::new();
    }

    let mut ranges = selection
        .filter(|selection| selection.track == TrackKind::Clip)
        .into_iter()
        .flat_map(|selection| selection.indices.iter())
        .filter_map(|index| clips.get(*index))
        .filter(|(start, end)| start.is_finite() && end.is_finite())
        .map(|(start, end)| (start.max(0.0), end.min(total_duration)))
        .filter(|(start, end)| end > start)
        .collect::<Vec<_>>();

    if ranges.is_empty()
        && let Some(&(start, end)) = clips
            .iter()
            .filter(|(start, end)| playhead >= *start && playhead < *end)
            .max_by(|left, right| left.0.total_cmp(&right.0))
    {
        ranges.push((start.max(0.0), end.min(total_duration)));
    }

    ranges.sort_by(|left, right| left.0.total_cmp(&right.0));
    let mut merged = Vec::<(f64, f64)>::with_capacity(ranges.len());
    for (start, end) in ranges {
        if let Some(previous) = merged.last_mut()
            && start <= previous.1
        {
            previous.1 = previous.1.max(end);
        } else {
            merged.push((start, end));
        }
    }
    merged
}

fn scene_layout_matches_at(scenes: &[SceneSegment], time: f64, choice: StudioLayout) -> bool {
    let scene = scenes
        .iter()
        .find(|scene| time >= scene.start && time < scene.end);
    match choice {
        StudioLayout::PictureInPicture => {
            scene.is_none_or(|scene| matches!(scene.mode, SceneMode::Default))
        }
        StudioLayout::CameraOnly => {
            scene.is_some_and(|scene| matches!(scene.mode, SceneMode::CameraOnly))
        }
        StudioLayout::ScreenLeft | StudioLayout::CameraLeft => scene.is_some_and(|scene| {
            matches!(scene.mode, SceneMode::SplitScreen | SceneMode::Floating)
                && scene.split_layout.is_some_and(|layout| {
                    layout.force_horizontal
                        && layout.camera_first == (choice == StudioLayout::CameraLeft)
                })
        }),
    }
}

fn scene_range_matches_layout(
    scenes: &[SceneSegment],
    range: (f64, f64),
    choice: StudioLayout,
) -> bool {
    let mut boundaries = vec![range.0, range.1];
    for scene in scenes {
        if scene.start < range.1 && scene.end > range.0 {
            boundaries.push(scene.start.clamp(range.0, range.1));
            boundaries.push(scene.end.clamp(range.0, range.1));
        }
    }
    boundaries.sort_by(f64::total_cmp);
    boundaries.dedup_by(|left, right| *left == *right);
    boundaries
        .windows(2)
        .all(|window| scene_layout_matches_at(scenes, (window[0] + window[1]) / 2.0, choice))
}

fn scene_layout_mismatch_ranges(
    scenes: &[SceneSegment],
    ranges: &[(f64, f64)],
    choice: StudioLayout,
) -> Vec<(f64, f64)> {
    let mut mismatches = Vec::<(f64, f64)>::new();
    for &(start, end) in ranges {
        if scene_range_matches_layout(scenes, (start, end), choice) {
            continue;
        }
        let mut boundaries = vec![start, end];
        for scene in scenes {
            if scene.start < end && scene.end > start {
                boundaries.push(scene.start.clamp(start, end));
                boundaries.push(scene.end.clamp(start, end));
            }
        }
        boundaries.sort_by(f64::total_cmp);
        boundaries.dedup_by(|left, right| *left == *right);
        for window in boundaries.windows(2) {
            if window[1] <= window[0]
                || scene_layout_matches_at(scenes, (window[0] + window[1]) / 2.0, choice)
            {
                continue;
            }
            if let Some(previous) = mismatches.last_mut()
                && previous.1 == window[0]
            {
                previous.1 = window[1];
            } else {
                mismatches.push((window[0], window[1]));
            }
        }
    }
    mismatches
}

fn scene_outside_ranges(scene: &SceneSegment, ranges: &[(f64, f64)]) -> Vec<SceneSegment> {
    let mut pieces = vec![scene.clone()];
    for &(range_start, range_end) in ranges {
        let mut next_pieces = Vec::with_capacity(pieces.len() + 1);
        for mut piece in pieces {
            if piece.end <= range_start || piece.start >= range_end {
                next_pieces.push(piece);
                continue;
            }

            if piece.start < range_start {
                let mut before = piece.clone();
                before.end = range_start;
                before.transition_out = 0.0;
                before.transition_in = before.transition_in.clamp(0.0, before.end - before.start);
                next_pieces.push(before);
            }
            if piece.end > range_end {
                piece.start = range_end;
                piece.transition_in = 0.0;
                piece.transition_out = piece.transition_out.clamp(0.0, piece.end - piece.start);
                next_pieces.push(piece);
            }
        }
        pieces = next_pieces;
    }
    pieces
}

pub(crate) fn edit_camera_only_color_correction_ranges(
    scenes: &mut Vec<SceneSegment>,
    ranges: &[(f64, f64)],
    fallback: &ColorCorrection,
    mut edit: impl FnMut(&mut ColorCorrection) -> bool,
) -> Option<bool> {
    let mut matched = false;
    let mut changed = false;
    let mut updated = Vec::with_capacity(scenes.len());

    for scene in scenes.iter() {
        let intersections = if matches!(scene.mode, SceneMode::CameraOnly) {
            ranges
                .iter()
                .filter_map(|&(start, end)| {
                    let start = start.max(scene.start);
                    let end = end.min(scene.end);
                    (end > start).then_some((start, end))
                })
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        if intersections.is_empty() {
            updated.push(scene.clone());
            continue;
        }
        matched = true;

        let mut changed_ranges = Vec::new();
        let mut grades = Vec::new();
        for &(start, end) in &intersections {
            let mut grade = scene
                .camera_color_correction
                .clone()
                .unwrap_or_else(|| fallback.clone());
            if edit(&mut grade) {
                changed_ranges.push((start, end));
                grades.push(grade);
                changed = true;
            }
        }

        if changed_ranges.is_empty() {
            updated.push(scene.clone());
            continue;
        }
        updated.extend(scene_outside_ranges(scene, &changed_ranges));
        for ((start, end), grade) in changed_ranges.into_iter().zip(grades) {
            let mut segment = scene.clone();
            segment.start = start;
            segment.end = end;
            segment.transition_in = if start == scene.start {
                scene.transition_in
            } else {
                0.0
            };
            segment.transition_out = if end == scene.end {
                scene.transition_out
            } else {
                0.0
            };
            segment.camera_color_correction = Some(grade);
            updated.push(segment);
        }
    }

    if changed {
        updated.sort_by(|left, right| left.start.total_cmp(&right.start));
        *scenes = updated;
    }
    matched.then_some(changed)
}

fn apply_scene_layout(
    scenes: &mut Vec<SceneSegment>,
    ranges: &[(f64, f64)],
    choice: StudioLayout,
) -> bool {
    let ranges = scene_layout_mismatch_ranges(scenes, ranges, choice);
    if ranges.is_empty() {
        return false;
    }

    let mut updated = scenes
        .iter()
        .flat_map(|scene| scene_outside_ranges(scene, &ranges))
        .collect::<Vec<_>>();
    for (start, end) in ranges {
        let (mode, split_layout) = match choice {
            StudioLayout::PictureInPicture => (SceneMode::Default, None),
            StudioLayout::CameraOnly => (SceneMode::CameraOnly, None),
            StudioLayout::ScreenLeft | StudioLayout::CameraLeft => (
                SceneMode::Floating,
                Some(cap_project::SplitLayout {
                    camera_first: choice == StudioLayout::CameraLeft,
                    force_horizontal: true,
                    camera_framing: SplitCameraFraming::Relaxed,
                    fit_screen: true,
                    ..Default::default()
                }),
            ),
        };
        updated.push(SceneSegment {
            start,
            end,
            mode,
            split_layout,
            camera_only_crop_zoom: (choice == StudioLayout::CameraOnly)
                .then_some(cap_project::DEFAULT_CAMERA_ONLY_CROP_ZOOM),
            camera_color_correction: (choice == StudioLayout::CameraOnly)
                .then_some(ColorCorrection::default()),
            transition_in: 0.0,
            transition_out: 0.0,
        });
    }
    updated.sort_by(|left, right| left.start.total_cmp(&right.start));
    *scenes = updated;
    true
}

fn scene_camera_layout(scene: &SceneSegment) -> Option<StudioLayout> {
    match scene.mode {
        SceneMode::SplitScreen | SceneMode::Floating => Some(
            if scene.split_layout.is_some_and(|layout| layout.camera_first) {
                StudioLayout::CameraLeft
            } else {
                StudioLayout::ScreenLeft
            },
        ),
        SceneMode::Default => Some(StudioLayout::PictureInPicture),
        SceneMode::CameraOnly => Some(StudioLayout::CameraOnly),
        SceneMode::HideCamera => None,
    }
}

fn camera_is_circle(camera: &Camera) -> bool {
    camera.rounding >= 100.
        && matches!(camera.rounding_type, CornerStyle::Rounded)
        && camera
            .aspect_ratio
            .map_or(matches!(camera.shape, CameraShape::Square), |ratio| {
                ratio.x > 0 && ratio.x == ratio.y
            })
}

fn set_camera_framing(camera: &mut Camera, shape: CameraShape, ratio: Option<XY<u32>>) {
    if camera_is_circle(camera) {
        camera.rounding = 25.;
    }
    camera.shape = shape;
    camera.aspect_ratio = ratio;
}

impl EditorWindow {
    pub(crate) fn camera_scene_edit_ranges(&self) -> Vec<(f64, f64)> {
        let Some(timeline) = self.project.timeline.as_ref() else {
            return Vec::new();
        };
        camera_scene_ranges(
            self.selection(),
            &editor_timeline::clip_output_ranges(timeline),
            self.playhead_time(),
            self.total_duration(),
        )
    }

    pub(crate) fn camera_only_scene_for_edit_ranges(
        &self,
        ranges: &[(f64, f64)],
    ) -> Option<&SceneSegment> {
        let timeline = self.project.timeline.as_ref()?;
        let time = camera_scene_time(self.playhead_time(), self.total_duration());
        let scenes = timeline.scene_segments.iter().filter(|scene| {
            matches!(scene.mode, SceneMode::CameraOnly)
                && ranges
                    .iter()
                    .any(|&(start, end)| scene.start < end && scene.end > start)
        });
        scenes
            .clone()
            .find(|scene| scene.start <= time && time < scene.end)
            .or_else(|| scenes.into_iter().next())
    }

    pub(crate) fn camera_scene_index_at(&self, playhead: f64) -> Option<usize> {
        let timeline = self.project.timeline.as_ref()?;
        let end = self.total_duration();
        camera_scene_at(&timeline.scene_segments, camera_scene_time(playhead, end))
    }

    fn camera_scene(&self) -> Option<&SceneSegment> {
        let index = self.camera_scene_index_at(self.playhead_time())?;
        self.project.timeline.as_ref()?.scene_segments.get(index)
    }

    pub(crate) fn camera_uses_split_layout(&self) -> bool {
        self.camera_scene()
            .is_some_and(|scene| matches!(scene.mode, SceneMode::SplitScreen | SceneMode::Floating))
    }

    pub(crate) fn render_camera_layout_controls(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme;
        let scene = self.camera_scene();
        let active = scene.map_or(Some(StudioLayout::PictureInPicture), scene_camera_layout);
        let framing = scene
            .and_then(|scene| scene.split_layout)
            .unwrap_or_default()
            .camera_framing;
        let mut column = div().flex().flex_col().gap(px(12.)).child(
            ui::Field::stacked(&theme, tr::main::studio_layout::editor_layout()).child(
                div().flex().flex_wrap().gap(px(6.)).children(
                    LAYOUT_CHOICES.iter().copied().enumerate().map(
                        |(index, (layout, label, icon))| {
                            ui::EditorButton::plain(
                                &theme,
                                SharedString::from(format!("camera-layout-{index}")),
                            )
                            .choice(&theme)
                            .pressed(active == Some(layout))
                            .left_icon(icon)
                            .label(label())
                            .height(px(30.))
                            .text_size(px(12.))
                            .on_click(cx.listener(
                                move |this, _, window, cx| {
                                    this.set_camera_scene_layout(layout, window, cx)
                                },
                            ))
                        },
                    ),
                ),
            ),
        );
        if self.camera_uses_split_layout() {
            column = column.child(
                ui::Field::stacked(&theme, tr::main::studio_layout::framing()).child(
                    div().flex().flex_wrap().gap(px(6.)).children(
                        FRAMING_CHOICES.iter().copied().enumerate().map(
                            |(index, (choice, label))| {
                                ui::EditorButton::plain(
                                    &theme,
                                    SharedString::from(format!("split-camera-framing-{index}")),
                                )
                                .choice(&theme)
                                .pressed(framing == choice)
                                .label(label())
                                .height(px(30.))
                                .text_size(px(12.))
                                .on_click(cx.listener(
                                    move |this, _, window, cx| {
                                        let time = this.playhead_time();
                                        this.edit_project_global_without_timeline(
                                            "split-camera-framing",
                                            window,
                                            cx,
                                            |project| {
                                                let Some(timeline) = project.timeline.as_mut()
                                                else {
                                                    return false;
                                                };
                                                let time =
                                                    camera_scene_time(time, timeline.duration());
                                                let Some(index) =
                                                    camera_scene_at(&timeline.scene_segments, time)
                                                else {
                                                    return false;
                                                };
                                                let segment = &mut timeline.scene_segments[index];
                                                if !matches!(
                                                    segment.mode,
                                                    SceneMode::SplitScreen | SceneMode::Floating
                                                ) {
                                                    return false;
                                                }
                                                let layout = segment
                                                    .split_layout
                                                    .get_or_insert_with(Default::default);
                                                if layout.camera_framing == choice {
                                                    return false;
                                                }
                                                layout.camera_framing = choice;
                                                true
                                            },
                                        );
                                    },
                                ))
                            },
                        ),
                    ),
                ),
            );
        }
        if scene.is_some_and(|scene| matches!(scene.mode, SceneMode::CameraOnly))
            && let Some(index) = self.camera_scene_index_at(self.playhead_time())
        {
            column = column.child(self.slider_field(
                tr::main::studio_layout::crop_zoom(),
                SliderKey::Panel(
                    crate::editor_panels::PanelSlider::SceneCameraOnlyZoom,
                    index,
                ),
                "zoom",
                cx,
            ));
        }
        if self.project.timeline.as_ref().is_some_and(|timeline| {
            timeline
                .scene_segments
                .iter()
                .any(|scene| scene.start > 0. || scene.end + 1e-9 < timeline.duration())
        }) {
            column = column.child(
                div()
                    .text_size(px(11.))
                    .text_color(Hsla::from(theme.editor.text_2))
                    .child(tr::main::studio_layout::at_playhead()),
            );
        }
        column.into_any_element()
    }

    fn set_camera_scene_layout(
        &mut self,
        choice: StudioLayout,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let total_duration = self.total_duration();
        let ranges = if self.project.timeline.is_none() && choice != StudioLayout::PictureInPicture
        {
            if total_duration.is_finite() && total_duration > 0.0 {
                vec![(0.0, total_duration)]
            } else {
                Vec::new()
            }
        } else {
            camera_scene_ranges(
                self.selection(),
                &self
                    .project
                    .timeline
                    .as_ref()
                    .map(editor_timeline::clip_output_ranges)
                    .unwrap_or_default(),
                self.playhead_time(),
                total_duration,
            )
        };
        if ranges.is_empty() {
            return;
        }
        let durations = self.clip_display_durations.clone();
        self.edit_project_global("camera-scene-layout", window, cx, |project| {
            if project.timeline.is_none() && choice != StudioLayout::PictureInPicture {
                crate::editor_edits::ensure_timeline(project, &durations);
            }
            let Some(timeline) = project.timeline.as_mut() else {
                return false;
            };
            if apply_scene_layout(&mut timeline.scene_segments, &ranges, choice) {
                true
            } else {
                false
            }
        });
    }

    pub(crate) fn camera_ratio_label(&self) -> SharedString {
        let camera = &self.style_control_project().camera;
        if camera_is_circle(camera) {
            return tr::editor::tabs::camera::shape::circle().into();
        }
        let label = match camera.aspect_ratio {
            Some(XY { x: 16, y: 9 }) => tr::editor::tabs::camera::framing::wide().to_string(),
            Some(XY { x: 4, y: 3 }) => tr::editor::tabs::camera::framing::standard().to_string(),
            Some(XY { x: 3, y: 4 }) => tr::editor::tabs::camera::framing::portrait().to_string(),
            Some(XY { x: 9, y: 16 }) => tr::editor::tabs::camera::framing::vertical().to_string(),
            Some(ratio) => format!(
                "{} {}:{}",
                tr::editor::tabs::camera::framing::custom(),
                ratio.x,
                ratio.y
            ),
            None => match camera.shape {
                CameraShape::Source => tr::editor::tabs::camera::shape::source().to_string(),
                CameraShape::Square => tr::editor::tabs::camera::shape::square().to_string(),
            },
        };
        label.into()
    }

    pub(crate) fn camera_ratio_items(&self, kind: SidebarMenu) -> Vec<ui::MenuItem> {
        let camera = &self.style_control_project().camera;
        let circle = camera_is_circle(camera);
        let is = |x, y| {
            !circle
                && camera
                    .aspect_ratio
                    .is_some_and(|ratio| ratio.x == x && ratio.y == y)
        };
        if kind == SidebarMenu::CameraMoreShape {
            vec![
                ui::MenuItem::new(tr::editor::tabs::camera::framing::back(), false),
                ui::MenuItem::new(tr::editor::tabs::camera::framing::portrait(), is(3, 4)),
                ui::MenuItem::new(tr::editor::tabs::camera::framing::vertical(), is(9, 16)),
                ui::MenuItem::new(tr::editor::tabs::camera::framing::custom(), false),
            ]
        } else {
            vec![
                ui::MenuItem::new(
                    tr::editor::tabs::camera::shape::source(),
                    camera.aspect_ratio.is_none() && matches!(camera.shape, CameraShape::Source),
                ),
                ui::MenuItem::new(tr::editor::tabs::camera::framing::wide(), is(16, 9)),
                ui::MenuItem::new(tr::editor::tabs::camera::framing::standard(), is(4, 3)),
                ui::MenuItem::new(
                    tr::editor::tabs::camera::shape::square(),
                    !circle
                        && camera.aspect_ratio.is_none()
                        && matches!(camera.shape, CameraShape::Square),
                ),
                ui::MenuItem::new(tr::editor::tabs::camera::shape::circle(), circle),
                ui::MenuItem::new(
                    tr::editor::tabs::camera::framing::more(),
                    !circle && camera.aspect_ratio.is_some() && !is(16, 9) && !is(4, 3),
                ),
            ]
        }
    }

    pub(crate) fn choose_camera_ratio(
        &mut self,
        kind: SidebarMenu,
        index: usize,
        anchor: Option<ui::MenuState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if (kind == SidebarMenu::CameraShape && index == 5)
            || (kind == SidebarMenu::CameraMoreShape && index == 0)
        {
            if let Some(state) = anchor {
                let next = if kind == SidebarMenu::CameraShape {
                    SidebarMenu::CameraMoreShape
                } else {
                    SidebarMenu::CameraShape
                };
                let items = self.camera_ratio_items(next);
                self.sidebar.menu = Some(OpenMenu {
                    kind: next,
                    state: ui::MenuState::new(state.origin, &items),
                });
            }
            cx.notify();
            return;
        }
        if kind == SidebarMenu::CameraMoreShape && index == 3 {
            self.open_camera_ratio_dialog(window, cx);
            return;
        }
        if kind == SidebarMenu::CameraShape && index == 4 {
            self.edit_project("camera-circle", window, cx, |project| {
                if camera_is_circle(&project.camera) {
                    return false;
                }
                crate::recording::apply_camera_preview_shape(
                    &mut project.camera,
                    crate::store::CameraShape::Round,
                );
                true
            });
            return;
        }
        let (shape, ratio) = match (kind, index) {
            (SidebarMenu::CameraShape, 0) => (CameraShape::Source, None),
            (SidebarMenu::CameraShape, 1) => (CameraShape::Source, Some(XY::new(16, 9))),
            (SidebarMenu::CameraShape, 2) => (CameraShape::Source, Some(XY::new(4, 3))),
            (SidebarMenu::CameraShape, 3) => (CameraShape::Square, None),
            (SidebarMenu::CameraMoreShape, 1) => (CameraShape::Source, Some(XY::new(3, 4))),
            (SidebarMenu::CameraMoreShape, 2) => (CameraShape::Source, Some(XY::new(9, 16))),
            _ => return,
        };
        self.edit_project("camera-aspect-ratio", window, cx, move |project| {
            set_camera_framing(&mut project.camera, shape, ratio);
            true
        });
    }

    pub(crate) fn render_camera_framing_controls(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme;
        div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(
                ui::Field::inline(&theme, tr::editor::tabs::camera::shape_label())
                    .child(div().flex_1().min_w_0().child(self.menu_select_owned(
                        SidebarMenu::CameraShape,
                        "camera-aspect-ratio".into(),
                        self.camera_ratio_label(),
                        cx,
                    )))
                    .child(
                        ui::EditorButton::plain(&theme, "camera-adjust-framing")
                            .tinted(&theme)
                            .choice(&theme)
                            .left_icon("icons/crop.svg")
                            .icon_size(px(14.))
                            .text_size(px(12.))
                            .label(tr::editor::tabs::camera::framing::adjust())
                            .pressed(self.camera_framing.crop_open)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.camera_framing.crop_open = !this.camera_framing.crop_open;
                                cx.notify();
                            })),
                    ),
            )
            .children(self.camera_framing.crop_open.then(|| {
                div()
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .child(self.slider_field(
                        tr::editor::tabs::camera::framing::horizontal(),
                        SliderKey::Camera(CameraSlider::CropX),
                        "%",
                        cx,
                    ))
                    .child(self.slider_field(
                        tr::editor::tabs::camera::framing::vertical_position(),
                        SliderKey::Camera(CameraSlider::CropY),
                        "%",
                        cx,
                    ))
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(Hsla::from(theme.editor.text_2))
                            .child(tr::editor::tabs::camera::framing::crop_hint()),
                    )
                    .child(
                        ui::Button::plain(
                            &theme,
                            "camera-framing-done",
                            ui::ButtonVariant::Gray,
                            ui::ButtonSize::Md,
                        )
                        .label(tr::editor::tabs::camera::framing::done())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.camera_framing.crop_open = false;
                            cx.notify();
                        })),
                    )
            }))
            .into_any_element()
    }

    fn open_camera_ratio_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ratio = self
            .style_control_project()
            .camera
            .aspect_ratio
            .unwrap_or(XY::new(4, 5));
        let inputs = [ratio.x, ratio.y].map(|value| {
            cx.new(|cx| {
                let mut input = ui::TextInputState::single_line(window, cx);
                input.set_text(value.to_string(), cx);
                input
            })
        });
        let subscriptions = inputs
            .iter()
            .map(|input| {
                cx.subscribe_in(
                    input,
                    window,
                    |this: &mut Self, _, event: &ui::TextInputEvent, window, cx| match event {
                        ui::TextInputEvent::Confirmed => this.apply_custom_camera_ratio(window, cx),
                        ui::TextInputEvent::Cancelled => this.close_camera_ratio_dialog(window, cx),
                        _ => cx.notify(),
                    },
                )
            })
            .collect();
        inputs[0].update(cx, |input, cx| input.focus_and_select_all(window, cx));
        self.camera_framing.dialog = Some(RatioDialog {
            width: inputs[0].clone(),
            height: inputs[1].clone(),
            error: false,
            _subscriptions: subscriptions,
        });
        cx.notify();
    }

    pub(crate) fn camera_ratio_dialog_tab(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(dialog) = &self.camera_framing.dialog {
            let target = if dialog.width.read(cx).focus_handle().is_focused(window) {
                &dialog.height
            } else {
                &dialog.width
            };
            target.update(cx, |input, cx| input.focus_and_select_all(window, cx));
        }
    }

    pub(crate) fn close_camera_ratio_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.camera_framing.dialog = None;
        window.focus(&self.focus_handle_for_menu(), cx);
        cx.notify();
    }

    fn apply_custom_camera_ratio(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.camera_framing.dialog.as_mut() else {
            return;
        };
        let Some(ratio) = parse_ratio(dialog.width.read(cx).text(), dialog.height.read(cx).text())
        else {
            dialog.error = true;
            cx.notify();
            return;
        };
        self.close_camera_ratio_dialog(window, cx);
        self.edit_project("camera-custom-ratio", window, cx, move |project| {
            set_camera_framing(&mut project.camera, CameraShape::Source, Some(ratio));
            true
        });
    }

    pub(crate) fn render_camera_ratio_dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let dialog = self.camera_framing.dialog.as_ref()?;
        let theme = self.theme;
        Some(
            div()
                .absolute()
                .inset_0()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .id("camera-ratio-backdrop")
                        .absolute()
                        .inset_0()
                        .size_full()
                        .bg(gpui::hsla(0., 0., 0., 0.35))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.close_camera_ratio_dialog(window, cx)
                        })),
                )
                .child(
                    div()
                        .id("camera-ratio-dialog")
                        .occlude()
                        .w(px(360.))
                        .p(px(20.))
                        .flex()
                        .flex_col()
                        .gap(px(18.))
                        .rounded(px(12.))
                        .border_1()
                        .border_color(Hsla::from(theme.editor.line))
                        .bg(Hsla::from(theme.editor.card))
                        .shadow(theme.editor.pop_shadow())
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            div()
                                .text_size(px(14.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(tr::editor::tabs::camera::framing::custom()),
                        )
                        .child(
                            div()
                                .flex()
                                .gap(px(12.))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .flex()
                                        .flex_col()
                                        .gap(px(6.))
                                        .child(tr::editor::tabs::camera::framing::width())
                                        .child(
                                            ui::TextInput::plain(
                                                &theme,
                                                "camera-ratio-width",
                                                &dialog.width,
                                            )
                                            .height(px(36.))
                                            .padding_x(px(10.))
                                            .radius(px(8.)),
                                        ),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .flex()
                                        .flex_col()
                                        .gap(px(6.))
                                        .child(tr::editor::tabs::camera::framing::height())
                                        .child(
                                            ui::TextInput::plain(
                                                &theme,
                                                "camera-ratio-height",
                                                &dialog.height,
                                            )
                                            .height(px(36.))
                                            .padding_x(px(10.))
                                            .radius(px(8.)),
                                        ),
                                ),
                        )
                        .children(dialog.error.then(|| {
                            div()
                                .text_size(px(12.))
                                .text_color(gpui::rgb(0xd92d20))
                                .child(tr::editor::tabs::camera::framing::invalid())
                        }))
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .gap(px(8.))
                                .child(
                                    ui::Button::plain(
                                        &theme,
                                        "camera-ratio-cancel",
                                        ui::ButtonVariant::Gray,
                                        ui::ButtonSize::Md,
                                    )
                                    .label(tr::common::cancel())
                                    .on_click(cx.listener(
                                        |this, _, window, cx| {
                                            this.close_camera_ratio_dialog(window, cx)
                                        },
                                    )),
                                )
                                .child(
                                    ui::Button::plain(
                                        &theme,
                                        "camera-ratio-apply",
                                        ui::ButtonVariant::Blue,
                                        ui::ButtonSize::Md,
                                    )
                                    .label(tr::editor::tabs::camera::framing::apply())
                                    .on_click(cx.listener(
                                        |this, _, window, cx| {
                                            this.apply_custom_camera_ratio(window, cx)
                                        },
                                    )),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scene_layout_matches_camera_only_and_split_modes() {
        let make_scene = |mode, split_layout| SceneSegment {
            start: 0.0,
            end: 1.0,
            mode,
            split_layout,
            camera_only_crop_zoom: None,
            camera_color_correction: None,
            transition_in: 0.0,
            transition_out: 0.0,
        };

        assert_eq!(
            scene_camera_layout(&make_scene(SceneMode::CameraOnly, None)),
            Some(StudioLayout::CameraOnly)
        );
        assert_eq!(
            scene_camera_layout(&make_scene(
                SceneMode::Floating,
                Some(cap_project::SplitLayout {
                    camera_first: true,
                    ..Default::default()
                })
            )),
            Some(StudioLayout::CameraLeft)
        );
        assert_eq!(
            scene_camera_layout(&make_scene(SceneMode::SplitScreen, None)),
            Some(StudioLayout::ScreenLeft)
        );
        assert_eq!(
            scene_camera_layout(&make_scene(SceneMode::HideCamera, None)),
            None
        );
        assert!(!StudioLayout::CameraOnly.is_split());
    }

    #[test]
    fn camera_layout_targets_selected_clips_or_the_clip_at_the_playhead() {
        let clips = [(0.0, 4.0), (4.0, 8.0), (8.0, 12.0)];
        let selected = crate::editor_edits::Selection {
            track: TrackKind::Clip,
            indices: vec![1],
        };
        assert_eq!(
            camera_scene_ranges(Some(&selected), &clips, 1.0, 12.0),
            [(4.0, 8.0)]
        );
        assert_eq!(camera_scene_ranges(None, &clips, 5.0, 12.0), [(4.0, 8.0)]);
    }

    #[test]
    fn camera_layout_preserves_matching_and_unselected_scene_framing() {
        let split_layout = cap_project::SplitLayout {
            camera_first: true,
            camera_zoom: 2.0,
            ..Default::default()
        };
        let mut scenes = vec![
            SceneSegment {
                start: 0.0,
                end: 4.0,
                mode: SceneMode::CameraOnly,
                camera_only_crop_zoom: Some(1.75),
                transition_in: 0.2,
                transition_out: 0.2,
                ..make_default_scene()
            },
            SceneSegment {
                start: 4.0,
                end: 8.0,
                mode: SceneMode::Default,
                ..make_default_scene()
            },
            SceneSegment {
                start: 8.0,
                end: 12.0,
                mode: SceneMode::Floating,
                split_layout: Some(split_layout),
                ..make_default_scene()
            },
        ];

        assert!(apply_scene_layout(
            &mut scenes,
            &[(0.0, 4.0), (4.0, 8.0)],
            StudioLayout::CameraOnly
        ));
        assert_eq!(scenes.len(), 3);
        assert_eq!(scenes[0].camera_only_crop_zoom, Some(1.75));
        assert_eq!(scenes[0].transition_in, 0.2);
        assert!(matches!(scenes[1].mode, SceneMode::CameraOnly));
        assert_eq!(
            scenes[1].camera_only_crop_zoom,
            Some(cap_project::DEFAULT_CAMERA_ONLY_CROP_ZOOM)
        );
        assert_eq!(
            scenes[1].camera_color_correction,
            Some(ColorCorrection::default())
        );
        assert!(matches!(scenes[2].mode, SceneMode::Floating));
        assert!(
            scenes[2]
                .split_layout
                .is_some_and(|layout| layout.camera_first && layout.camera_zoom == 2.0)
        );
        assert!(!apply_scene_layout(
            &mut scenes,
            &[(0.0, 4.0)],
            StudioLayout::CameraOnly
        ));
    }

    #[test]
    fn camera_color_correction_edit_only_changes_the_selected_part_of_a_scene() {
        let inherited = ColorCorrection::cinematic();
        let manually_selected = ColorCorrection::default();
        let mut scenes = vec![SceneSegment {
            start: 0.0,
            end: 10.0,
            mode: SceneMode::CameraOnly,
            camera_color_correction: None,
            ..make_default_scene()
        }];

        assert_eq!(
            edit_camera_only_color_correction_ranges(
                &mut scenes,
                &[(0.0, 4.0)],
                &inherited,
                |grade| {
                    *grade = manually_selected.clone();
                    true
                },
            ),
            Some(true)
        );
        assert_eq!(scenes.len(), 2);
        assert_eq!((scenes[0].start, scenes[0].end), (0.0, 4.0));
        assert_eq!(
            scenes[0].camera_color_correction,
            Some(manually_selected.clone())
        );
        assert_eq!((scenes[1].start, scenes[1].end), (4.0, 10.0));
        assert!(scenes[1].camera_color_correction.is_none());
        assert_eq!(
            edit_camera_only_color_correction_ranges(
                &mut scenes,
                &[(0.0, 4.0)],
                &inherited,
                |grade| *grade != manually_selected,
            ),
            Some(false)
        );
        assert_eq!(scenes.len(), 2);
    }

    #[test]
    fn replacing_a_clipped_scene_keeps_outer_transitions_outside_the_clip() {
        let mut scenes = vec![SceneSegment {
            start: 0.0,
            end: 12.0,
            mode: SceneMode::Default,
            transition_in: 0.4,
            transition_out: 0.4,
            ..make_default_scene()
        }];
        assert!(apply_scene_layout(
            &mut scenes,
            &[(4.0, 8.0)],
            StudioLayout::CameraOnly
        ));
        assert_eq!(scenes.len(), 3);
        assert_eq!((scenes[0].start, scenes[0].end), (0.0, 4.0));
        assert_eq!(
            (scenes[0].transition_in, scenes[0].transition_out),
            (0.4, 0.0)
        );
        assert_eq!((scenes[1].start, scenes[1].end), (4.0, 8.0));
        assert_eq!(
            (scenes[1].transition_in, scenes[1].transition_out),
            (0.0, 0.0)
        );
        assert_eq!((scenes[2].start, scenes[2].end), (8.0, 12.0));
        assert_eq!(
            (scenes[2].transition_in, scenes[2].transition_out),
            (0.0, 0.4)
        );
    }

    fn make_default_scene() -> SceneSegment {
        SceneSegment {
            start: 0.0,
            end: 1.0,
            mode: SceneMode::Default,
            split_layout: None,
            camera_only_crop_zoom: None,
            camera_color_correction: None,
            transition_in: 0.0,
            transition_out: 0.0,
        }
    }

    #[test]
    fn a_recorded_circle_stays_recognizable_and_can_return_to_a_square() {
        let mut camera = Camera::default();
        crate::recording::apply_camera_preview_shape(&mut camera, crate::store::CameraShape::Round);
        let mut reopened: Camera =
            serde_json::from_value(serde_json::to_value(camera).unwrap()).unwrap();
        assert!(camera_is_circle(&reopened));
        assert_eq!(reopened.framing_aspect_ratio([1920., 1080.]), 1.);
        set_camera_framing(&mut reopened, CameraShape::Square, None);
        assert!(!camera_is_circle(&reopened));
        assert_eq!(reopened.framing_aspect_ratio([1920., 1080.]), 1.);
    }

    #[test]
    fn custom_ratio_rejects_invalid_input_and_preserves_dimensions() {
        for (width, height) in [
            ("0", "1"),
            ("1", "0"),
            ("101", "50"),
            ("NaN", "3"),
            ("1", "100"),
            ("5.5", "4"),
        ] {
            assert!(parse_ratio(width, height).is_none());
        }
        let ratio = parse_ratio(" 4 ", "5").unwrap();
        assert_eq!((ratio.x, ratio.y), (4, 5));
        assert!(parse_ratio("1", "4").is_some());
        assert!(parse_ratio("4", "1").is_some());
    }
}
