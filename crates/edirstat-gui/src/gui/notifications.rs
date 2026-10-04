// Based on egui-notify (MIT License - Copyright (c) 2022 ItsEthra)

use std::time::Duration;

use eframe::egui::{
    self, Align, Color32, Context, CornerRadius, FontId, FontSelection, Id, LayerId, Order, Pos2,
    Rect, Shadow, Stroke, TextWrapMode, Vec2, WidgetText, pos2, text::TextWrapping, vec2,
};

pub(crate) const TOAST_WIDTH: f32 = 180.0;
pub(crate) const TOAST_HEIGHT: f32 = 34.0;

const ERROR_COLOR: Color32 = Color32::from_rgb(200, 90, 90);
const INFO_COLOR: Color32 = Color32::from_rgb(150, 200, 210);
const WARNING_COLOR: Color32 = Color32::from_rgb(230, 220, 140);
const SUCCESS_COLOR: Color32 = Color32::from_rgb(140, 230, 140);

/// Anchor where to show toasts
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Anchor {
    /// Top right corner.
    #[default]
    TopRight,
    /// Top left corner.
    TopLeft,
    /// Bottom right corner.
    BottomRight,
    /// Bottom left corner.
    BottomLeft,
}

impl Anchor {
    #[inline]
    #[must_use]
    pub(crate) const fn anim_side(self) -> f32 {
        match self {
            Self::TopRight | Self::BottomRight => 1.0,
            Self::TopLeft | Self::BottomLeft => -1.0,
        }
    }

    #[must_use]
    pub(crate) fn screen_corner(self, sc: Pos2, margin: Vec2) -> Pos2 {
        let mut out = match self {
            Self::TopRight => pos2(sc.x, 0.0),
            Self::TopLeft => pos2(0.0, 0.0),
            Self::BottomRight => sc,
            Self::BottomLeft => pos2(0.0, sc.y),
        };
        self.apply_margin(&mut out, margin);
        out
    }

    pub(crate) fn apply_margin(self, pos: &mut Pos2, margin: Vec2) {
        match self {
            Self::TopRight => {
                pos.x -= margin.x;
                pos.y += margin.y;
            }
            Self::TopLeft => {
                pos.x += margin.x;
                pos.y += margin.y;
            }
            Self::BottomRight => {
                pos.x -= margin.x;
                pos.y -= margin.y;
            }
            Self::BottomLeft => {
                pos.x += margin.x;
                pos.y -= margin.y;
            }
        }
    }
}

/// Level of importance
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub enum ToastLevel {
    #[default]
    Info,
    Warning,
    Error,
    Success,
    None,
    Custom(String, Color32),
}

/// State of the toast
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastState {
    /// Toast is appearing
    Appear,
    /// Toast is disappearing
    Disappear,
    /// Toast has disappeared
    Disappeared,
    /// Toast is idling
    Idle,
}

impl ToastState {
    /// Returns `true` if the toast is appearing
    #[must_use]
    pub const fn appearing(self) -> bool {
        matches!(self, Self::Appear)
    }

    /// Returns `true` if the toast is disappearing
    #[must_use]
    pub const fn disappearing(self) -> bool {
        matches!(self, Self::Disappear)
    }

    /// Returns `true` if the toast has disappeared
    #[must_use]
    pub const fn disappeared(self) -> bool {
        matches!(self, Self::Disappeared)
    }

    /// Returns `true` if the toast is idling
    #[must_use]
    pub const fn idling(self) -> bool {
        matches!(self, Self::Idle)
    }
}

/// Container for options for initializing toasts
pub struct ToastOptions {
    duration: Option<Duration>,
    level: ToastLevel,
    closable: bool,
    show_progress_bar: bool,
}

impl Default for ToastOptions {
    fn default() -> Self {
        Self {
            duration: Some(Duration::from_millis(3500)),
            level: ToastLevel::None,
            closable: true,
            show_progress_bar: true,
        }
    }
}

/// Single notification or *toast*
pub struct Toast {
    pub(crate) level: ToastLevel,
    pub(crate) caption: WidgetText,
    // (initial, current)
    pub(crate) duration: Option<(f32, f32)>,
    pub(crate) height: f32,
    pub(crate) width: f32,
    pub(crate) closable: bool,
    pub(crate) show_progress_bar: bool,

    pub(crate) state: ToastState,
    pub(crate) value: f32,
}

impl Toast {
    fn new(caption: impl Into<WidgetText>, options: ToastOptions) -> Self {
        Self {
            caption: caption.into(),
            height: TOAST_HEIGHT,
            width: TOAST_WIDTH,
            duration: options.duration.map(|dur| {
                let max_dur = dur.as_secs_f32();
                (max_dur, max_dur)
            }),
            closable: options.closable,
            show_progress_bar: options.show_progress_bar,
            level: options.level,
            value: 0.0,
            state: ToastState::Appear,
        }
    }

    /// Creates new basic toast, can be closed by default.
    #[must_use]
    pub fn basic(caption: impl Into<WidgetText>) -> Self {
        Self::new(caption, ToastOptions::default())
    }

    /// Creates new success toast, can be closed by default.
    #[must_use]
    pub fn success(caption: impl Into<WidgetText>) -> Self {
        Self::new(
            caption,
            ToastOptions {
                level: ToastLevel::Success,
                ..ToastOptions::default()
            },
        )
    }

    /// Creates new info toast, can be closed by default.
    #[must_use]
    pub fn info(caption: impl Into<WidgetText>) -> Self {
        Self::new(
            caption,
            ToastOptions {
                level: ToastLevel::Info,
                ..ToastOptions::default()
            },
        )
    }

    /// Creates new warning toast, can be closed by default.
    #[must_use]
    pub fn warning(caption: impl Into<WidgetText>) -> Self {
        Self::new(
            caption,
            ToastOptions {
                level: ToastLevel::Warning,
                ..ToastOptions::default()
            },
        )
    }

    /// Creates new error toast, cannot be closed by default.
    #[must_use]
    pub fn error(caption: impl Into<WidgetText>) -> Self {
        Self::new(
            caption,
            ToastOptions {
                closable: false,
                level: ToastLevel::Error,
                ..ToastOptions::default()
            },
        )
    }

    /// Creates new custom toast, can be closed by default.
    #[must_use]
    pub fn custom(caption: impl Into<WidgetText>, level: ToastLevel) -> Self {
        Self::new(
            caption,
            ToastOptions {
                level,
                ..ToastOptions::default()
            },
        )
    }

    /// Set the options with a [`ToastOptions`]
    pub fn options(&mut self, options: ToastOptions) -> &mut Self {
        self.closable(options.closable);
        self.duration(options.duration);
        self.level(options.level);
        self
    }

    /// Change the level of the toast
    pub fn level(&mut self, level: ToastLevel) -> &mut Self {
        self.level = level;
        self
    }

    /// Can the user close the toast?
    pub const fn closable(&mut self, closable: bool) -> &mut Self {
        self.closable = closable;
        self
    }

    /// Should a progress bar be shown?
    pub const fn show_progress_bar(&mut self, show_progress_bar: bool) -> &mut Self {
        self.show_progress_bar = show_progress_bar;
        self
    }

    /// In what time should the toast expire? Set to `None` for no expiry.
    pub fn duration(&mut self, duration: impl Into<Option<Duration>>) -> &mut Self {
        if let Some(duration) = duration.into() {
            let max_dur = duration.as_secs_f32();
            self.duration = Some((max_dur, max_dur));
        } else {
            self.duration = None;
        }
        self
    }

    /// Toast's box height
    pub const fn height(&mut self, height: f32) -> &mut Self {
        self.height = height;
        self
    }

    /// Toast's box width
    pub const fn width(&mut self, width: f32) -> &mut Self {
        self.width = width;
        self
    }

    /// Dismiss this toast
    pub const fn dismiss(&mut self) {
        self.state = ToastState::Disappear;
    }

    #[must_use]
    pub(crate) fn calc_anchored_rect(&self, pos: Pos2, anchor: Anchor) -> Rect {
        match anchor {
            Anchor::TopRight => Rect {
                min: pos2(pos.x - self.width, pos.y),
                max: pos2(pos.x, pos.y + self.height),
            },
            Anchor::TopLeft => Rect {
                min: pos,
                max: pos + vec2(self.width, self.height),
            },
            Anchor::BottomRight => Rect {
                min: pos - vec2(self.width, self.height),
                max: pos,
            },
            Anchor::BottomLeft => Rect {
                min: pos2(pos.x, pos.y - self.height),
                max: pos2(pos.x + self.width, pos.y),
            },
        }
    }

    pub(crate) fn adjust_next_pos(&self, pos: &mut Pos2, anchor: Anchor, spacing: f32) {
        match anchor {
            Anchor::TopRight | Anchor::TopLeft => pos.y += self.height + spacing,
            Anchor::BottomRight | Anchor::BottomLeft => pos.y -= self.height + spacing,
        }
    }
}

/// Main notifications collector.
pub struct Toasts {
    #[allow(clippy::struct_field_names)]
    toasts: Vec<Toast>,
    anchor: Anchor,
    margin: Vec2,
    spacing: f32,
    padding: Vec2,
    reverse: bool,
    speed: f32,
    font: Option<FontId>,
    shadow: Option<Shadow>,
    held: bool,
}

impl Toasts {
    /// Creates new [`Toasts`] instance.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            anchor: Anchor::TopRight,
            margin: vec2(8.0, 8.0),
            toasts: Vec::new(),
            spacing: 8.0,
            padding: vec2(10.0, 10.0),
            held: false,
            speed: 4.0,
            reverse: false,
            font: None,
            shadow: None,
        }
    }

    /// Adds new toast to the collection.
    /// By default adds toast at the end of the list, can be changed with `self.reverse`.
    pub fn add(&mut self, toast: Toast) -> &mut Toast {
        if self.reverse {
            self.toasts.insert(0, toast);
            &mut self.toasts[0]
        } else {
            self.toasts.push(toast);
            let last_idx = self.toasts.len() - 1;
            &mut self.toasts[last_idx]
        }
    }

    /// Dismisses the oldest toast
    pub fn dismiss_oldest_toast(&mut self) {
        if let Some(toast) = self.toasts.first_mut() {
            toast.dismiss();
        }
    }

    /// Dismisses the most recent toast
    pub fn dismiss_latest_toast(&mut self) {
        if let Some(toast) = self.toasts.last_mut() {
            toast.dismiss();
        }
    }

    /// Dismisses all toasts
    pub fn dismiss_all_toasts(&mut self) {
        for toast in &mut self.toasts {
            toast.dismiss();
        }
    }

    /// Returns the number of toast items.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.toasts.len()
    }

    /// Returns `true` if there are no toast items.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.toasts.is_empty()
    }

    /// Shortcut for adding a toast with level `Success`.
    pub fn success(&mut self, caption: impl Into<WidgetText>) -> &mut Toast {
        self.add(Toast::success(caption))
    }

    /// Shortcut for adding a toast with level `Info`.
    pub fn info(&mut self, caption: impl Into<WidgetText>) -> &mut Toast {
        self.add(Toast::info(caption))
    }

    /// Shortcut for adding a toast with level `Warning`.
    pub fn warning(&mut self, caption: impl Into<WidgetText>) -> &mut Toast {
        self.add(Toast::warning(caption))
    }

    /// Shortcut for adding a toast with level `Error`.
    pub fn error(&mut self, caption: impl Into<WidgetText>) -> &mut Toast {
        self.add(Toast::error(caption))
    }

    /// Shortcut for adding a toast with no level.
    pub fn basic(&mut self, caption: impl Into<WidgetText>) -> &mut Toast {
        self.add(Toast::basic(caption))
    }

    /// Shortcut for adding a toast with custom `level`.
    pub fn custom(
        &mut self,
        caption: impl Into<WidgetText>,
        level_string: String,
        level_color: egui::Color32,
    ) -> &mut Toast {
        self.add(Toast::custom(
            caption,
            ToastLevel::Custom(level_string, level_color),
        ))
    }

    /// Should toasts be added in reverse order?
    #[must_use]
    pub const fn reverse(mut self, reverse: bool) -> Self {
        self.reverse = reverse;
        self
    }

    /// Where toasts should appear.
    #[must_use]
    pub const fn with_anchor(mut self, anchor: Anchor) -> Self {
        self.anchor = anchor;
        self
    }

    /// Sets spacing between adjacent toasts.
    #[must_use]
    pub const fn with_spacing(mut self, spacing: f32) -> Self {
        self.spacing = spacing;
        self
    }

    /// Margin or distance from screen to toasts' bounding boxes
    #[must_use]
    pub const fn with_margin(mut self, margin: Vec2) -> Self {
        self.margin = margin;
        self
    }

    /// Enables the use of a shadow for toasts.
    #[must_use]
    pub const fn with_shadow(mut self, shadow: Shadow) -> Self {
        self.shadow = Some(shadow);
        self
    }

    /// Padding or distance from toasts' bounding boxes to inner contents.
    #[must_use]
    pub const fn with_padding(mut self, padding: Vec2) -> Self {
        self.padding = padding;
        self
    }

    /// Changes the default font used for all toasts.
    #[must_use]
    pub fn with_default_font(mut self, font: FontId) -> Self {
        self.font = Some(font);
        self
    }

    /// Displays toast queue
    #[allow(clippy::cast_precision_loss)]
    pub fn show(&mut self, ctx: &Context) {
        let Self {
            anchor,
            margin,
            spacing,
            padding,
            toasts,
            held,
            speed,
            ..
        } = self;

        let mut pos = anchor.screen_corner(ctx.input(|i| i.content_rect().max), *margin);
        let p = ctx.layer_painter(LayerId::new(Order::Foreground, Id::new("toasts")));

        // `held` used to prevent sticky removal
        if ctx.input(|i| i.pointer.primary_released()) {
            *held = false;
        }

        let visuals = ctx.global_style().visuals.widgets.noninteractive;
        let mut update = false;

        toasts.retain_mut(|toast| {
            // Start disappearing expired toasts
            if let Some((_initial_d, current_d)) = toast.duration
                && current_d <= 0.0
            {
                toast.state = ToastState::Disappear;
            }

            let anim_offset = toast.width * (1.0 - ease_in_cubic(toast.value));
            pos.x = anim_offset.mul_add(anchor.anim_side(), pos.x);
            let rect = toast.calc_anchored_rect(pos, *anchor);

            if let Some((_, d)) = toast.duration.as_mut() {
                // Check if we hover over the toast and if true don't decrease the duration
                let hover_pos = ctx.input(|i| i.pointer.hover_pos());
                let is_outside_rect = hover_pos.is_none_or(|pos| !rect.contains(pos));

                if is_outside_rect && toast.state.idling() {
                    *d -= ctx.input(|i| i.stable_dt);
                    update = true;
                }
            }

            let caption_galley = toast.caption.clone().into_galley_impl(
                ctx,
                ctx.global_style().as_ref(),
                TextWrapping::from_wrap_mode_and_width(TextWrapMode::Extend, f32::INFINITY),
                FontSelection::Default,
                Align::LEFT,
            );

            let (caption_width, caption_height) =
                (caption_galley.rect.width(), caption_galley.rect.height());

            let line_count = caption_galley.rows.len().max(1);
            let icon_width = caption_height / line_count as f32;
            let rounding = CornerRadius::same(4);

            // Create toast icon
            let icon_font = FontId::proportional(icon_width);
            let icon_galley =
                match &toast.level {
                    ToastLevel::Info => {
                        Some(ctx.fonts_mut(|f| {
                            f.layout("ℹ".into(), icon_font, INFO_COLOR, f32::INFINITY)
                        }))
                    }
                    ToastLevel::Warning => Some(ctx.fonts_mut(|f| {
                        f.layout("⚠".into(), icon_font, WARNING_COLOR, f32::INFINITY)
                    })),
                    ToastLevel::Error => Some(ctx.fonts_mut(|f| {
                        f.layout("！".into(), icon_font, ERROR_COLOR, f32::INFINITY)
                    })),
                    ToastLevel::Success => Some(ctx.fonts_mut(|f| {
                        f.layout("✅".into(), icon_font, SUCCESS_COLOR, f32::INFINITY)
                    })),
                    ToastLevel::Custom(s, c) => {
                        Some(ctx.fonts_mut(|f| f.layout(s.clone(), icon_font, *c, f32::INFINITY)))
                    }
                    ToastLevel::None => None,
                };

            let (action_width, action_height) =
                icon_galley.as_ref().map_or((0.0, 0.0), |icon_galley| {
                    (icon_galley.rect.width(), icon_galley.rect.height())
                });

            // Create closing cross
            let cross_galley = if toast.closable {
                let cross_fid = FontId::proportional(icon_width);
                let cross_galley = ctx.fonts_mut(|f| {
                    f.layout(
                        "❌".into(),
                        cross_fid,
                        visuals.fg_stroke.color,
                        f32::INFINITY,
                    )
                });
                Some(cross_galley)
            } else {
                None
            };

            let (cross_width, cross_height) =
                cross_galley.as_ref().map_or((0.0, 0.0), |cross_galley| {
                    (cross_galley.rect.width(), cross_galley.rect.height())
                });

            let icon_x_padding = (0.0, padding.x);
            let cross_x_padding = (padding.x, 0.0);

            let icon_width_padded = if icon_width == 0.0 {
                0.0
            } else {
                icon_width + icon_x_padding.0 + icon_x_padding.1
            };
            let cross_width_padded = if cross_width == 0.0 {
                0.0
            } else {
                cross_width + cross_x_padding.0 + cross_x_padding.1
            };

            toast.width = padding
                .x
                .mul_add(2.0, icon_width_padded + caption_width + cross_width_padded);
            toast.height = padding
                .y
                .mul_add(2.0, action_height.max(caption_height).max(cross_height));

            // Required due to positioning of the next toast
            pos.x = anim_offset.mul_add(-anchor.anim_side(), pos.x);

            // Draw shadow
            if let Some(shadow) = self.shadow {
                let s = shadow.as_shape(rect, rounding);
                p.add(s);
            }

            // Draw background
            p.rect_filled(rect, rounding, visuals.bg_fill);

            // Paint icon
            if let Some((icon_galley, true)) =
                icon_galley.zip(Some(toast.level != ToastLevel::None))
            {
                let oy = toast.height / 2.0 - action_height / 2.0;
                let ox = padding.x + icon_x_padding.0;
                p.galley(
                    rect.min + vec2(ox, oy),
                    icon_galley,
                    visuals.fg_stroke.color,
                );
            }

            // Paint caption
            let oy = toast.height / 2.0 - caption_height / 2.0;
            let o_from_icon = if action_width == 0.0 {
                0.0
            } else {
                action_width + icon_x_padding.1
            };
            let o_from_cross = if cross_width == 0.0 {
                0.0
            } else {
                cross_width + cross_x_padding.0
            };
            let ox =
                (toast.width / 2.0 - caption_width / 2.0) + o_from_icon / 2.0 - o_from_cross / 2.0;
            p.galley(
                rect.min + vec2(ox, oy),
                caption_galley,
                visuals.fg_stroke.color,
            );

            // Paint cross
            if let Some(cross_galley) = cross_galley {
                let cross_rect = cross_galley.rect;
                let oy = toast.height / 2.0 - cross_height / 2.0;
                let ox = toast.width - cross_width - cross_x_padding.1 - padding.x;
                let cross_pos = rect.min + vec2(ox, oy);
                p.galley(cross_pos, cross_galley, Color32::BLACK);

                let screen_cross = Rect {
                    max: cross_pos + cross_rect.max.to_vec2(),
                    min: cross_pos,
                };

                if let Some(pos) = ctx.input(|i| i.pointer.press_origin())
                    && screen_cross.contains(pos)
                    && !*held
                {
                    toast.dismiss();
                    *held = true;
                }
            }

            // Draw duration
            if toast.show_progress_bar
                && let Some((initial, current)) = toast.duration
                && !toast.state.disappearing()
            {
                p.line_segment(
                    [
                        rect.min + vec2(0.0, toast.height),
                        rect.max - vec2((1.0 - (current / initial)) * toast.width, 0.0),
                    ],
                    Stroke::new(4.0, visuals.fg_stroke.color),
                );
            }

            toast.adjust_next_pos(&mut pos, *anchor, *spacing);

            // Animations
            if toast.state.appearing() {
                update = true;
                toast.value = ctx.input(|i| i.stable_dt).mul_add(*speed, toast.value);

                if toast.value >= 1.0 {
                    toast.value = 1.0;
                    toast.state = ToastState::Idle;
                }
            } else if toast.state.disappearing() {
                update = true;
                toast.value = ctx.input(|i| i.stable_dt).mul_add(-*speed, toast.value);

                if toast.value <= 0.0 {
                    toast.state = ToastState::Disappeared;
                }
            }

            // Remove disappeared toasts
            !toast.state.disappeared()
        });

        if update {
            ctx.request_repaint();
        }
    }
}

impl Default for Toasts {
    fn default() -> Self {
        Self::new()
    }
}

fn ease_in_cubic(x: f32) -> f32 {
    1.0 - (1.0 - x).powi(3)
}

/// Global toast manager instance anchored to the bottom-right with standard margins.
pub static TOASTS: std::sync::LazyLock<parking_lot::Mutex<Toasts>> =
    std::sync::LazyLock::new(|| {
        parking_lot::Mutex::new(
            Toasts::new()
                .with_anchor(Anchor::BottomRight)
                .with_margin(egui::vec2(10.0, 30.0)),
        )
    });

/// Show a success toast notification lasting 4 seconds.
pub fn toast_success(message: impl Into<egui::WidgetText>) {
    TOASTS
        .lock()
        .success(message)
        .duration(Some(Duration::from_secs(4)));
}

/// Show an informational toast notification lasting 4 seconds.
pub fn toast_info(message: impl Into<egui::WidgetText>) {
    TOASTS
        .lock()
        .info(message)
        .duration(Some(Duration::from_secs(4)));
}

/// Show a warning toast notification lasting 8 seconds.
pub fn toast_warning(message: impl Into<egui::WidgetText>) {
    TOASTS
        .lock()
        .warning(message)
        .duration(Some(Duration::from_secs(8)));
}

/// Show an error toast notification lasting 16 seconds.
pub fn toast_error(message: impl Into<egui::WidgetText>) {
    TOASTS
        .lock()
        .error(message)
        .duration(Some(Duration::from_secs(16)));
}

/// Render all active toasts in the current egui frame.
pub fn show_toasts(ctx: &egui::Context) {
    TOASTS.lock().show(ctx);
}

// Module-level convenience aliases
pub fn success(message: impl Into<egui::WidgetText>) {
    toast_success(message);
}

pub fn info(message: impl Into<egui::WidgetText>) {
    toast_info(message);
}

pub fn warning(message: impl Into<egui::WidgetText>) {
    toast_warning(message);
}

pub fn error(message: impl Into<egui::WidgetText>) {
    toast_error(message);
}

pub fn show(ctx: &egui::Context) {
    show_toasts(ctx);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::pedantic)]
    fn test_anchor_anim_side_and_corners() {
        assert_eq!(Anchor::TopRight.anim_side(), 1.0);
        assert_eq!(Anchor::BottomRight.anim_side(), 1.0);
        assert_eq!(Anchor::TopLeft.anim_side(), -1.0);
        assert_eq!(Anchor::BottomLeft.anim_side(), -1.0);

        let screen_size = pos2(1920.0, 1080.0);
        let margin = vec2(10.0, 20.0);

        let tr = Anchor::TopRight.screen_corner(screen_size, margin);
        assert_eq!(tr, pos2(1910.0, 20.0));

        let tl = Anchor::TopLeft.screen_corner(screen_size, margin);
        assert_eq!(tl, pos2(10.0, 20.0));

        let br = Anchor::BottomRight.screen_corner(screen_size, margin);
        assert_eq!(br, pos2(1910.0, 1060.0));

        let bl = Anchor::BottomLeft.screen_corner(screen_size, margin);
        assert_eq!(bl, pos2(10.0, 1060.0));
    }

    #[test]
    fn test_toast_levels_and_options() {
        let t_info = Toast::info("Information text");
        assert_eq!(t_info.level, ToastLevel::Info);
        assert!(t_info.closable);

        let t_err = Toast::error("Error text");
        assert_eq!(t_err.level, ToastLevel::Error);
        assert!(!t_err.closable);

        let t_warn = Toast::warning("Warning text");
        assert_eq!(t_warn.level, ToastLevel::Warning);

        let t_succ = Toast::success("Success text");
        assert_eq!(t_succ.level, ToastLevel::Success);

        let t_cust = Toast::custom(
            "Custom text",
            ToastLevel::Custom("Tag".into(), Color32::RED),
        );
        assert_eq!(t_cust.level, ToastLevel::Custom("Tag".into(), Color32::RED));
    }

    #[test]
    fn test_toasts_collection_lifecycle() {
        let mut toasts = Toasts::new();
        assert!(toasts.is_empty());
        assert_eq!(toasts.len(), 0);

        toasts.info("First");
        toasts.warning("Second");
        toasts.error("Third");

        assert_eq!(toasts.len(), 3);
        assert!(!toasts.is_empty());

        toasts.dismiss_oldest_toast();
        assert!(toasts.toasts[0].state.disappearing());

        toasts.dismiss_latest_toast();
        assert!(toasts.toasts[2].state.disappearing());

        toasts.dismiss_all_toasts();
        for t in &toasts.toasts {
            assert!(t.state.disappearing());
        }
    }

    #[test]
    fn test_global_toast_helpers() {
        toast_info("Test global info");
        toast_success("Test global success");
        toast_warning("Test global warning");
        toast_error("Test global error");

        let mut lock = TOASTS.lock();
        assert!(lock.len() >= 4);
        lock.dismiss_all_toasts();
    }
}
