//! 物理ピクセルの座標と矩形。スクリーンショットと同じ座標系 (仮想デスクトップ、原点は負になりうる)。

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

impl Point {
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }

    /// 線分 a-b からこの点までの距離 (ピクセル)。停止要因の「実ユーザーのマウス移動」の判定に使う。
    pub fn distance_to_segment(self, a: Point, b: Point) -> f64 {
        let (px, py) = (f64::from(self.x), f64::from(self.y));
        let (ax, ay) = (f64::from(a.x), f64::from(a.y));
        let (bx, by) = (f64::from(b.x), f64::from(b.y));
        let (dx, dy) = (bx - ax, by - ay);
        let len2 = dx * dx + dy * dy;
        let t = if len2 == 0.0 {
            0.0
        } else {
            (((px - ax) * dx + (py - ay) * dy) / len2).clamp(0.0, 1.0)
        };
        let (cx, cy) = (ax + t * dx, ay + t * dy);
        ((px - cx).powi(2) + (py - cy).powi(2)).sqrt()
    }
}

/// 左上と幅・高さで表す矩形。右端と下端は含まない (`x <= px < x + w`)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: u32, h: u32) -> Self {
        Self { x, y, w, h }
    }

    fn right(&self) -> i64 {
        i64::from(self.x) + i64::from(self.w)
    }

    fn bottom(&self) -> i64 {
        i64::from(self.y) + i64::from(self.h)
    }

    pub fn contains(&self, p: Point) -> bool {
        i64::from(p.x) >= i64::from(self.x)
            && i64::from(p.x) < self.right()
            && i64::from(p.y) >= i64::from(self.y)
            && i64::from(p.y) < self.bottom()
    }

    /// `other` がこの矩形に丸ごと含まれるか。
    pub fn contains_rect(&self, other: &Rect) -> bool {
        other.w > 0
            && other.h > 0
            && i64::from(other.x) >= i64::from(self.x)
            && i64::from(other.y) >= i64::from(self.y)
            && other.right() <= self.right()
            && other.bottom() <= self.bottom()
    }

    pub fn intersects(&self, other: &Rect) -> bool {
        i64::from(self.x) < other.right()
            && i64::from(other.x) < self.right()
            && i64::from(self.y) < other.bottom()
            && i64::from(other.y) < self.bottom()
    }

    /// 重なる部分。重ならなければ None
    pub fn intersection(&self, other: &Rect) -> Option<Rect> {
        let x1 = i64::from(self.x).max(i64::from(other.x));
        let y1 = i64::from(self.y).max(i64::from(other.y));
        let x2 = self.right().min(other.right());
        let y2 = self.bottom().min(other.bottom());
        (x2 > x1 && y2 > y1)
            .then(|| Rect::new(x1 as i32, y1 as i32, (x2 - x1) as u32, (y2 - y1) as u32))
    }

    pub fn center(&self) -> Point {
        Point::new(
            (i64::from(self.x) + i64::from(self.w) / 2) as i32,
            (i64::from(self.y) + i64::from(self.h) / 2) as i32,
        )
    }

    pub fn is_empty(&self) -> bool {
        self.w == 0 || self.h == 0
    }

    /// この矩形から `other` との重なりを除いた残り (D-077)。重ならなければ自分 1 つ、覆われれば空。
    /// 上の帯、下の帯、重なりの高さの左と右の順で最大 4 つ (幅か高さが 0 のものは入れない)。
    pub fn subtract(&self, other: &Rect) -> Vec<Rect> {
        let Some(cut) = self.intersection(other) else {
            return if self.is_empty() {
                Vec::new()
            } else {
                vec![*self]
            };
        };
        let (x1, y1) = (i64::from(self.x), i64::from(self.y));
        let (x2, y2) = (self.right(), self.bottom());
        let (cx1, cy1) = (i64::from(cut.x), i64::from(cut.y));
        let (cx2, cy2) = (cut.right(), cut.bottom());
        let band = |l: i64, t: i64, r: i64, b: i64| {
            (r > l && b > t).then(|| Rect::new(l as i32, t as i32, (r - l) as u32, (b - t) as u32))
        };
        [
            band(x1, y1, x2, cy1),
            band(x1, cy2, x2, y2),
            band(x1, cy1, cx1, cy2),
            band(cx2, cy1, x2, cy2),
        ]
        .into_iter()
        .flatten()
        .collect()
    }
}

/// `base` から `covers` を順に引いた残り (D-077。見えている部分)。
pub fn visible_parts(base: Rect, covers: &[Rect]) -> Vec<Rect> {
    let mut parts = if base.is_empty() {
        Vec::new()
    } else {
        vec![base]
    };
    for c in covers {
        if parts.is_empty() {
            break;
        }
        parts = parts.iter().flat_map(|p| p.subtract(c)).collect();
    }
    parts
}

/// 矩形の並びの外接矩形。空なら None
pub fn bounding(parts: &[Rect]) -> Option<Rect> {
    let first = parts.first()?;
    let (mut x1, mut y1) = (i64::from(first.x), i64::from(first.y));
    let (mut x2, mut y2) = (first.right(), first.bottom());
    for p in &parts[1..] {
        x1 = x1.min(i64::from(p.x));
        y1 = y1.min(i64::from(p.y));
        x2 = x2.max(p.right());
        y2 = y2.max(p.bottom());
    }
    Some(Rect::new(
        x1 as i32,
        y1 as i32,
        (x2 - x1) as u32,
        (y2 - y1) as u32,
    ))
}

impl fmt::Display for Rect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{},{},{},{}", self.x, self.y, self.w, self.h)
    }
}

impl FromStr for Rect {
    type Err = String;

    /// `"x,y,w,h"` (空白は無視)。幅と高さは 1 以上。
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = s.split(',').map(str::trim).collect();
        if parts.len() != 4 {
            return Err(format!("領域は \"x,y,w,h\" の形で書く: {s:?}"));
        }
        let num = |t: &str| {
            t.parse::<i64>()
                .map_err(|_| format!("数ではない: {t:?} (in {s:?})"))
        };
        let (x, y, w, h) = (
            num(parts[0])?,
            num(parts[1])?,
            num(parts[2])?,
            num(parts[3])?,
        );
        let x = i32::try_from(x).map_err(|_| format!("x が範囲外: {s:?}"))?;
        let y = i32::try_from(y).map_err(|_| format!("y が範囲外: {s:?}"))?;
        if w <= 0 || h <= 0 {
            return Err(format!("幅と高さは 1 以上: {s:?}"));
        }
        let w = u32::try_from(w).map_err(|_| format!("幅が範囲外: {s:?}"))?;
        let h = u32::try_from(h).map_err(|_| format!("高さが範囲外: {s:?}"))?;
        if i64::from(x) + i64::from(w) > i64::from(i32::MAX)
            || i64::from(y) + i64::from(h) > i64::from(i32::MAX)
        {
            return Err(format!("領域が座標の範囲を超える: {s:?}"));
        }
        Ok(Rect::new(x, y, w, h))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_parse_and_display_roundtrip() {
        let r: Rect = " -10, 20 ,300,400".parse().unwrap();
        assert_eq!(r, Rect::new(-10, 20, 300, 400));
        assert_eq!(r.to_string().parse::<Rect>().unwrap(), r);
    }

    #[test]
    fn rect_parse_rejects_bad_input() {
        for bad in ["", "1,2,3", "1,2,3,4,5", "a,2,3,4", "1,2,0,4", "1,2,3,-4"] {
            assert!(bad.parse::<Rect>().is_err(), "{bad}");
        }
    }

    #[test]
    fn rect_contains_is_half_open() {
        let r = Rect::new(0, 0, 10, 10);
        assert!(r.contains(Point::new(0, 0)));
        assert!(r.contains(Point::new(9, 9)));
        assert!(!r.contains(Point::new(10, 9)));
        assert!(!r.contains(Point::new(-1, 0)));
    }

    #[test]
    fn rect_contains_rect_and_intersects() {
        let outer = Rect::new(0, 0, 100, 100);
        assert!(outer.contains_rect(&Rect::new(10, 10, 90, 90)));
        assert!(!outer.contains_rect(&Rect::new(10, 10, 91, 90)));
        assert!(outer.intersects(&Rect::new(99, 99, 5, 5)));
        assert!(!outer.intersects(&Rect::new(100, 0, 5, 5)));
    }

    fn area(parts: &[Rect]) -> u64 {
        parts.iter().map(|r| u64::from(r.w) * u64::from(r.h)).sum()
    }

    /// 矩形の引き算 (D-077): 重ならない、隣り合う、含まれる、角、中央の穴
    #[test]
    fn rect_subtract_covers_disjoint_contained_corner_and_hole() {
        let r = Rect::new(0, 0, 100, 100);
        // 重ならない
        assert_eq!(r.subtract(&Rect::new(200, 200, 10, 10)), vec![r]);
        // 隣り合う (右端に接する。右端は含まないので重ならない)
        assert_eq!(r.subtract(&Rect::new(100, 0, 10, 100)), vec![r]);
        assert_eq!(r.subtract(&Rect::new(0, 100, 100, 10)), vec![r]);
        // 含まれる (ちょうど同じ、はみ出して覆う)
        assert!(r.subtract(&r).is_empty());
        assert!(r.subtract(&Rect::new(-10, -10, 200, 200)).is_empty());
        // 角 (右下): 上の帯と、重なりの高さの左
        let corner = r.subtract(&Rect::new(50, 50, 100, 100));
        assert_eq!(
            corner,
            vec![Rect::new(0, 0, 100, 50), Rect::new(0, 50, 50, 50)]
        );
        assert_eq!(area(&corner), 100 * 100 - 50 * 50);
        // 中央の穴: 上、下、左、右の 4 つで、面積の和は元から穴を引いた値
        let hole = r.subtract(&Rect::new(10, 20, 30, 40));
        assert_eq!(
            hole,
            vec![
                Rect::new(0, 0, 100, 20),
                Rect::new(0, 60, 100, 40),
                Rect::new(0, 20, 10, 40),
                Rect::new(40, 20, 60, 40),
            ]
        );
        assert_eq!(area(&hole), 100 * 100 - 30 * 40);
        // 残りどうしは重ならない
        for (i, a) in hole.iter().enumerate() {
            for b in &hole[i + 1..] {
                assert!(!a.intersects(b), "{a} と {b}");
            }
        }
        // 負の原点でも同じ
        let neg = Rect::new(-100, -100, 50, 50).subtract(&Rect::new(-90, -90, 10, 10));
        assert_eq!(neg.len(), 4);
        assert_eq!(area(&neg), 50 * 50 - 10 * 10);
    }

    /// 覆いを順に引いて 1 ピクセルの帯だけ残る。覆いが矩形の外まで伸びても残りは矩形の内側
    #[test]
    fn visible_parts_leaves_one_pixel_strip() {
        let base = Rect::new(0, 0, 100, 100);
        let covers = [Rect::new(-50, -50, 120, 300), Rect::new(70, -50, 29, 300)];
        assert_eq!(visible_parts(base, &covers), vec![Rect::new(99, 0, 1, 100)]);
        // 最後の 1 ピクセルの帯も覆えば空
        let all = [covers[0], covers[1], Rect::new(99, 0, 1, 100)];
        assert!(visible_parts(base, &all).is_empty());
        // 覆いが無ければ自分 1 つ。空の矩形は空
        assert_eq!(visible_parts(base, &[]), vec![base]);
        assert!(visible_parts(Rect::new(0, 0, 0, 10), &[]).is_empty());
    }

    #[test]
    fn bounding_of_parts() {
        assert_eq!(bounding(&[]), None);
        let one = Rect::new(5, 6, 7, 8);
        assert_eq!(bounding(&[one]), Some(one));
        assert_eq!(
            bounding(&[Rect::new(-10, 20, 5, 5), Rect::new(30, -5, 10, 10)]),
            Some(Rect::new(-10, -5, 50, 30))
        );
    }

    #[test]
    fn distance_to_segment() {
        let a = Point::new(0, 0);
        let b = Point::new(10, 0);
        assert_eq!(Point::new(5, 3).distance_to_segment(a, b), 3.0);
        assert_eq!(Point::new(13, 4).distance_to_segment(a, b), 5.0);
        assert_eq!(Point::new(3, 4).distance_to_segment(a, a), 5.0);
    }
}
