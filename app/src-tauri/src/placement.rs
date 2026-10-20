//! 오버레이 창 위치 계산 (순수 함수).
//!
//! 좌표는 모두 **물리 픽셀**(Per-Monitor V2 DPI 인식 프로세스의 화면 좌표)이다. CSS(논리) px는
//! 오버레이 창 안의 좌표에만 쓴다. `scale`은 메뉴를 띄울 모니터의 배율(1.0 = 100%)이다.

use crate::events::{GuidePosition, Point};

/// 오버레이 창 논리 크기 (CSS px, 정사각형)
pub const OVERLAY_CSS_SIZE: f64 = 480.0;
/// 메뉴 중심 = 창 중심 (CSS px)
pub const CENTER_CSS: f64 = OVERLAY_CSS_SIZE / 2.0;
/// 바깥 고리 반지름 (CSS px)
pub const RING_RADIUS_CSS: f64 = 120.0;
/// 고리와 안내 줄 사이 (CSS px)
pub const GUIDE_GAP_CSS: f64 = 12.0;
/// 안내 줄 높이 (CSS px)
pub const GUIDE_HEIGHT_CSS: f64 = 28.0;

/// 모니터 작업 영역. `right`, `bottom`은 포함하지 않는다(Win32 RECT와 같다).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rect {
    /// 위치와 크기로 만든다 (tauri `PhysicalRect` 변환용)
    pub fn from_origin_size(x: i32, y: i32, width: u32, height: u32) -> Self {
        let w = i32::try_from(width).unwrap_or(i32::MAX);
        let h = i32::try_from(height).unwrap_or(i32::MAX);
        Self {
            left: x,
            top: y,
            right: x.saturating_add(w),
            bottom: y.saturating_add(h),
        }
    }

    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    /// 창 왼쪽 위 (물리 px)
    pub window_x: i32,
    pub window_y: i32,
    /// 메뉴 중심 (물리 px). 커서가 가장자리에 있으면 커서와 다르다.
    pub center_x: i32,
    pub center_y: i32,
    pub guide: GuidePosition,
    /// 계산에 쓴 배율 (잘못된 값이면 1.0으로 바뀐다)
    pub scale: f64,
}

fn sane_scale(scale: f64) -> f64 {
    if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    }
}

fn px(css: f64, scale: f64) -> i32 {
    (css * scale).ceil() as i32
}

/// 한 축에서 반지름 r 원이 [lo, hi) 안에 들어오도록 중심을 민다.
/// 작업 영역이 원보다 작으면 가운데에 둔다.
fn clamp_axis(v: i32, lo: i32, hi: i32, r: i32) -> i32 {
    let min = i64::from(lo) + i64::from(r);
    let max = i64::from(hi) - i64::from(r);
    if min > max {
        ((i64::from(lo) + i64::from(hi)) / 2) as i32
    } else {
        i64::from(v).clamp(min, max) as i32
    }
}

/// 커서 위치에 메뉴를 놓는다.
///
/// 1. 고리(반지름 120×scale) 원이 작업 영역 안에 들어오도록 중심을 민다.
/// 2. 아래쪽에 고리+12+28이 들어가지 않으면 안내 줄을 위(`Above`)로 둔다. 위에도 안 들어가면 아래.
/// 3. 창 위치 = 중심 − 240×scale.
pub fn place_overlay(cursor: (i32, i32), work_area: Rect, scale: f64) -> Placement {
    let scale = sane_scale(scale);
    let r = px(RING_RADIUS_CSS, scale);
    let center_x = clamp_axis(cursor.0, work_area.left, work_area.right, r);
    let center_y = clamp_axis(cursor.1, work_area.top, work_area.bottom, r);

    let guide_extent = px(RING_RADIUS_CSS + GUIDE_GAP_CSS + GUIDE_HEIGHT_CSS, scale);
    let fits_below = i64::from(center_y) + i64::from(guide_extent) <= i64::from(work_area.bottom);
    let fits_above = i64::from(center_y) - i64::from(guide_extent) >= i64::from(work_area.top);
    let guide = if fits_below || !fits_above {
        GuidePosition::Below
    } else {
        GuidePosition::Above
    };

    let half = (CENTER_CSS * scale).round() as i32;
    Placement {
        window_x: center_x.saturating_sub(half),
        window_y: center_y.saturating_sub(half),
        center_x,
        center_y,
        guide,
        scale,
    }
}

/// 화면 물리 좌표 → 오버레이 창 기준 CSS px (제스처 `radial:mbutton_up`용)
pub fn to_overlay_css(screen: (i32, i32), window_origin: (i32, i32), scale: f64) -> Point {
    let scale = sane_scale(scale);
    Point {
        x: f64::from(screen.0 - window_origin.0) / scale,
        y: f64::from(screen.1 - window_origin.1) / scale,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 1920×1080 모니터, 아래 작업 표시줄 40px
    const WA: Rect = Rect {
        left: 0,
        top: 0,
        right: 1920,
        bottom: 1040,
    };

    #[test]
    fn middle_of_screen_is_unchanged() {
        let p = place_overlay((960, 500), WA, 1.0);
        assert_eq!((p.center_x, p.center_y), (960, 500));
        assert_eq!((p.window_x, p.window_y), (720, 260));
        assert_eq!(p.guide, GuidePosition::Below);
    }

    #[test]
    fn top_left_corner() {
        let p = place_overlay((5, 5), WA, 1.0);
        assert_eq!((p.center_x, p.center_y), (120, 120));
        assert_eq!((p.window_x, p.window_y), (-120, -120));
        assert_eq!(p.guide, GuidePosition::Below);
    }

    #[test]
    fn top_right_corner() {
        let p = place_overlay((1919, 0), WA, 1.0);
        assert_eq!((p.center_x, p.center_y), (1800, 120));
        assert_eq!(p.guide, GuidePosition::Below);
    }

    #[test]
    fn bottom_left_corner_guide_above() {
        let p = place_overlay((0, 1039), WA, 1.0);
        assert_eq!((p.center_x, p.center_y), (120, 920));
        // 920 + 160 > 1040 → 위
        assert_eq!(p.guide, GuidePosition::Above);
    }

    #[test]
    fn bottom_right_corner_guide_above() {
        let p = place_overlay((1915, 1035), WA, 1.0);
        assert_eq!((p.center_x, p.center_y), (1800, 920));
        assert_eq!(p.guide, GuidePosition::Above);
        assert_eq!((p.window_x, p.window_y), (1560, 680));
    }

    #[test]
    fn near_bottom_but_ring_fits_guide_above() {
        let p = place_overlay((960, 900), WA, 1.0);
        assert_eq!(p.center_y, 900);
        assert_eq!(p.guide, GuidePosition::Above);
        // 경계: 880 + 160 = 1040 → 아래에 딱 들어감
        let p = place_overlay((960, 880), WA, 1.0);
        assert_eq!(p.guide, GuidePosition::Below);
        let p = place_overlay((960, 881), WA, 1.0);
        assert_eq!(p.guide, GuidePosition::Above);
    }

    #[test]
    fn scale_150_percent() {
        // 2880×1620 물리 px 모니터 (배율 150%), 작업 표시줄 60px
        let wa = Rect {
            left: 0,
            top: 0,
            right: 2880,
            bottom: 1560,
        };
        let p = place_overlay((3, 3), wa, 1.5);
        assert_eq!((p.center_x, p.center_y), (180, 180));
        assert_eq!((p.window_x, p.window_y), (-180, -180));
        let p = place_overlay((2879, 1559), wa, 1.5);
        assert_eq!((p.center_x, p.center_y), (2700, 1380));
        // 1380 + 240 > 1560
        assert_eq!(p.guide, GuidePosition::Above);
        let p = place_overlay((1000, 1320), wa, 1.5);
        assert_eq!(p.guide, GuidePosition::Below);
        let p = place_overlay((1000, 1321), wa, 1.5);
        assert_eq!(p.guide, GuidePosition::Above);
    }

    #[test]
    fn negative_coordinates_left_monitor() {
        // 주 모니터 왼쪽의 보조 모니터
        let wa = Rect {
            left: -1920,
            top: 0,
            right: 0,
            bottom: 1080,
        };
        let p = place_overlay((-1, 500), wa, 1.0);
        assert_eq!((p.center_x, p.center_y), (-120, 500));
        assert_eq!((p.window_x, p.window_y), (-360, 260));
        let p = place_overlay((-1920, 0), wa, 1.0);
        assert_eq!((p.center_x, p.center_y), (-1800, 120));
    }

    #[test]
    fn monitor_above_with_negative_y() {
        let wa = Rect {
            left: 0,
            top: -1080,
            right: 1920,
            bottom: 0,
        };
        let p = place_overlay((100, -2), wa, 1.25);
        assert_eq!(p.center_y, -150);
        assert_eq!(p.center_x, 150);
        assert_eq!(p.guide, GuidePosition::Above);
    }

    #[test]
    fn tiny_work_area_centers() {
        let wa = Rect {
            left: 0,
            top: 0,
            right: 200,
            bottom: 100,
        };
        let p = place_overlay((10, 10), wa, 1.0);
        assert_eq!((p.center_x, p.center_y), (100, 50));
        assert_eq!(p.guide, GuidePosition::Below);
    }

    #[test]
    fn bad_scale_falls_back_to_one() {
        let p = place_overlay((960, 500), WA, 0.0);
        assert_eq!(p.scale, 1.0);
        let p = place_overlay((960, 500), WA, f64::NAN);
        assert_eq!(p.scale, 1.0);
        assert_eq!((p.window_x, p.window_y), (720, 260));
    }

    #[test]
    fn css_conversion_round_trip_center() {
        let p = place_overlay((5, 5), WA, 1.5);
        let c = to_overlay_css((p.center_x, p.center_y), (p.window_x, p.window_y), p.scale);
        assert_eq!(c, Point { x: 240.0, y: 240.0 });
        let c = to_overlay_css(
            (p.window_x + 30, p.window_y + 60),
            (p.window_x, p.window_y),
            1.5,
        );
        assert_eq!(c, Point { x: 20.0, y: 40.0 });
    }

    #[test]
    fn rect_from_origin_size() {
        let r = Rect::from_origin_size(-1920, 0, 1920, 1040);
        assert_eq!(r.right, 0);
        assert_eq!(r.bottom, 1040);
        assert!(r.contains(-1, 0));
        assert!(!r.contains(0, 0));
    }
}
