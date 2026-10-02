//! GUI 无关的 2D 几何与颜色原语（y 轴向下，与主流即时模式 GUI 一致）。

/// 平面上的点。
#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

/// 尺寸。
#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Size {
    pub w: f32,
    pub h: f32,
}

impl Size {
    pub const fn new(w: f32, h: f32) -> Self {
        Self { w, h }
    }
}

/// 矩形（左上角原点）。
#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Rect {
    pub pos: Point,
    pub size: Size,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self {
            pos: Point::new(x, y),
            size: Size::new(w, h),
        }
    }
    pub fn from_min_max(a: Point, b: Point) -> Self {
        Self {
            pos: a,
            size: Size::new((b.x - a.x).max(0.0), (b.y - a.y).max(0.0)),
        }
    }
    pub fn right(&self) -> f32 {
        self.pos.x + self.size.w
    }
    pub fn bottom(&self) -> f32 {
        self.pos.y + self.size.h
    }
    pub fn center(&self) -> Point {
        Point::new(self.pos.x + self.size.w * 0.5, self.pos.y + self.size.h * 0.5)
    }
    pub fn contains(&self, p: Point) -> bool {
        p.x >= self.pos.x && p.x <= self.right() && p.y >= self.pos.y && p.y <= self.bottom()
    }
    pub fn translate(&self, dx: f32, dy: f32) -> Rect {
        Rect::new(self.pos.x + dx, self.pos.y + dy, self.size.w, self.size.h)
    }
    /// 返回 rect 落在 self 内的裁剪结果（可能为空矩形）。
    pub fn intersect(&self, rect: Rect) -> Rect {
        let x0 = self.pos.x.max(rect.pos.x);
        let y0 = self.pos.y.max(rect.pos.y);
        let x1 = self.right().min(rect.right());
        let y1 = self.bottom().min(rect.bottom());
        Rect::new(x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0))
    }
}

/// RGBA 颜色，分量 0.0..=1.0。
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const fn rgb(r: f32, g: f32, b: f32) -> Self {
        Self { r, g, b, a: 1.0 }
    }
    pub const fn rgba(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }
    pub const WHITE: Color = Color::rgb(1.0, 1.0, 1.0);
    pub const BLACK: Color = Color::rgb(0.05, 0.05, 0.08);
    pub const GRAY: Color = Color::rgb(0.5, 0.5, 0.55);
    pub const LIGHT_GRAY: Color = Color::rgb(0.78, 0.78, 0.8);
    pub const DARK_BG: Color = Color::rgb(0.13, 0.13, 0.16);
    pub const ACCENT: Color = Color::rgb(0.2, 0.55, 0.95);
    pub const GREEN: Color = Color::rgb(0.25, 0.7, 0.35);
    pub const YELLOW: Color = Color::rgb(0.9, 0.75, 0.2);
    pub const RED: Color = Color::rgb(0.85, 0.3, 0.3);
    pub const TRANSPARENT: Color = Color::rgba(0.0, 0.0, 0.0, 0.0);

    /// 由 HSV 的色相生成饱和度/亮度固定的颜色（示例视图用）。
    pub fn from_hue(hue: f32) -> Color {
        let h = hue.rem_euclid(360.0) / 60.0;
        let i = h.floor();
        let f = h - i;
        let (r, g, b) = match i as i64 % 6 {
            0 => (1.0, f, 0.0),
            1 => (1.0 - f, 1.0, 0.0),
            2 => (0.0, 1.0, f),
            3 => (0.0, 1.0 - f, 1.0),
            4 => (f, 0.0, 1.0),
            _ => (1.0, 0.0, 1.0 - f),
        };
        // 亮度 0.75、饱和度 0.65 的近似
        let mix = |c: f32| 0.75 * (0.35 + 0.65 * c);
        Color::rgb(mix(r), mix(g), mix(b))
    }
}
