//! 撮影した画像の png 保存と、画面の待ち (D-081) の画像の比較。

use std::fs::OpenOptions;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};

use crate::device::Image;
use crate::run::ImageWriter;

pub fn encode_png(img: &Image) -> Result<Vec<u8>, String> {
    let expected = (img.width as usize) * (img.height as usize) * 4;
    if img.rgba.len() != expected {
        return Err(format!(
            "画素の数が合わない ({} != {expected})",
            img.rgba.len()
        ));
    }
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, img.width, img.height);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().map_err(|e| e.to_string())?;
        w.write_image_data(&img.rgba).map_err(|e| e.to_string())?;
        w.finish().map_err(|e| e.to_string())?;
    }
    Ok(out)
}

/// 画像を `scale` (0 より大きく 1 以下) に縮める (D-053)。箱型の平均で、各出力の点は元の対応する範囲の平均 (四捨五入)。
/// 大きさは四捨五入で、最小 1x1。1 なら元のまま。
pub fn downscale(img: &Image, scale: f64) -> Image {
    if !(scale.is_finite() && scale > 0.0 && scale < 1.0) || img.width == 0 || img.height == 0 {
        return img.clone();
    }
    let dst_w = ((f64::from(img.width) * scale).round() as u32).max(1);
    let dst_h = ((f64::from(img.height) * scale).round() as u32).max(1);
    let (sw, sh) = (img.width as usize, img.height as usize);
    let (dw, dh) = (dst_w as usize, dst_h as usize);
    // 出力の点 d に対応する元の範囲 [d * s / dn, (d + 1) * s / dn) (少なくとも 1 点)
    let span = |d: usize, s: usize, dn: usize| -> (usize, usize) {
        let a = d * s / dn;
        let b = ((d + 1) * s / dn).max(a + 1).min(s);
        (a, b)
    };
    let mut rgba = Vec::with_capacity(dw * dh * 4);
    for dy in 0..dh {
        let (y0, y1) = span(dy, sh, dh);
        for dx in 0..dw {
            let (x0, x1) = span(dx, sw, dw);
            let mut sum = [0u64; 4];
            for y in y0..y1 {
                for x in x0..x1 {
                    let i = (y * sw + x) * 4;
                    for (c, s) in sum.iter_mut().enumerate() {
                        *s += u64::from(img.rgba[i + c]);
                    }
                }
            }
            let n = ((y1 - y0) * (x1 - x0)) as u64;
            for s in sum {
                rgba.push(((s + n / 2) / n).min(255) as u8);
            }
        }
    }
    Image {
        width: dst_w,
        height: dst_h,
        rgba,
    }
}

/// 実際に使う縮尺 (D-072): `scale` と `max_edge / 長辺` の小さい方。縮める向きだけで、
/// `max_edge` が無い、長辺が 0、長辺が `max_edge` 以下なら `scale` のまま。長辺は撮る矩形の `max(w, h)` (物理ピクセル)。
/// `Some(0)` は呼び出し側の形の確認で弾く (debug では panic)。
pub fn effective_scale(scale: f64, max_edge: Option<u32>, w: u32, h: u32) -> f64 {
    debug_assert!(
        max_edge != Some(0),
        "max_edge は 1 以上 (D-072。形の確認で弾く)"
    );
    let long = w.max(h);
    match max_edge {
        Some(m) if long > 0 && long > m => scale.min(f64::from(m) / f64::from(long)),
        _ => scale,
    }
}

/// 画面の待ち (D-081) で比べる前に縮める長辺の上限 (物理ピクセル)。
pub const DIFF_EDGE: u32 = 256;

/// 画面の待ち (D-081) で、点が「変わった」と数える RGB の差の下限 (これを超えたら変わった)。
pub const PIXEL_TOLERANCE: u8 = 16;

/// 比較用に長辺を `DIFF_EDGE` 以下へ縮めた画像 (D-081)。長辺が `DIFF_EDGE` 以下なら元のまま。
pub fn thumb(img: &Image) -> Image {
    downscale(
        img,
        effective_scale(1.0, Some(DIFF_EDGE), img.width, img.height),
    )
}

/// 2 つの画像の変わった点の割合 (0.0〜1.0。D-081)。RGB のどれかの差の絶対値が `PIXEL_TOLERANCE` を
/// 超える点を数える (アルファは見ない)。大きさが違えば 1.0、点が無ければ 0.0。
pub fn frame_diff(a: &Image, b: &Image) -> f64 {
    if a.width != b.width || a.height != b.height || a.rgba.len() != b.rgba.len() {
        return 1.0;
    }
    let total = a.rgba.len() / 4;
    if total == 0 {
        return 0.0;
    }
    let changed = a
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .zip(b.rgba.as_chunks::<4>().0)
        .filter(|(p, q)| (0..3).any(|c| p[c].abs_diff(q[c]) > PIXEL_TOLERANCE))
        .count();
    changed as f64 / total as f64
}

/// `wait_stable` と settle の「静か」: 差が閾値以下 (D-081)。
pub fn is_quiet(diff: f64, threshold: f64) -> bool {
    diff <= threshold
}

/// `wait_change` の「変わった」: 差が閾値を超えた (D-081)。
pub fn is_changed(diff: f64, threshold: f64) -> bool {
    diff > threshold
}

/// png のバイト列を読み戻す (テストと検証用)。
pub fn decode_png(bytes: &[u8]) -> Result<Image, String> {
    let dec = png::Decoder::new(std::io::Cursor::new(bytes));
    let mut reader = dec.read_info().map_err(|e| e.to_string())?;
    let size = reader
        .output_buffer_size()
        .ok_or("出力の大きさが分からない")?;
    let mut buf = vec![0u8; size];
    let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
    if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
        return Err(format!(
            "RGBA 8 ビットではない: {:?} {:?}",
            info.color_type, info.bit_depth
        ));
    }
    buf.truncate(info.buffer_size());
    Ok(Image {
        width: info.width,
        height: info.height,
        rgba: buf,
    })
}

/// PNG のファイルの先頭 8 バイトの署名。
const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

/// `\\?\` か `\\.\` で始まるか (D-075)。区切りは `\` と `/` の両方 (`//?/C:/s` も真)。
/// 字面の正規化と部品の比較を逃れる書き方なので、撮影の保存先と設定の save_dirs の両方で断る。
pub fn is_verbatim_or_device(path: &str) -> bool {
    let b = path.as_bytes();
    let sep = |c: u8| c == b'\\' || c == b'/';
    b.len() >= 4 && sep(b[0]) && sep(b[1]) && (b[2] == b'?' || b[2] == b'.') && sep(b[3])
}

/// 撮影の保存先のパスの形の規則 (D-075。設定に関係なく常に効く)。順に、`\\?\` / `\\.\`、
/// ドライブの `X:` の後の `:` (代替データストリーム)、拡張子が `.png` (大文字小文字を問わない) か。
pub fn check_save_path_shape(path: &str) -> Result<(), String> {
    if is_verbatim_or_device(path) {
        return Err(format!(
            "撮影の保存先に \\\\?\\ や \\\\.\\ で始まるパスは使えない: {path}"
        ));
    }
    let b = path.as_bytes();
    let rest = if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
        &path[2..]
    } else {
        path
    };
    if rest.contains(':') {
        return Err(format!(
            "撮影の保存先のファイル名に : は使えない (代替データストリーム): {path}"
        ));
    }
    let is_png = Path::new(path)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("png"));
    if !is_png {
        return Err(format!("撮影の保存先は .png のファイルに限る: {path}"));
    }
    Ok(())
}

/// 保存先を `cwd` に足し (絶対やドライブ付きのパスは `join` が置き換える)、字面で `.` を捨てて `..` で 1 つ戻る
/// (根と接頭辞より上には戻らない)。ファイルシステムには触れず、リンクもたどらない (D-075)。
/// ドライブ相対の `D:x.png` は絶対にならない (save_dirs のある層では外れて断られる = 安全側)。
pub fn normalize_save_path(path: &str, cwd: &Path) -> PathBuf {
    let joined = cwd.join(path);
    let mut out: Vec<Component> = Vec::new();
    for c in joined.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(out.last(), Some(Component::Normal(_))) {
                    out.pop();
                } else if !matches!(out.last(), Some(Component::Prefix(_) | Component::RootDir)) {
                    // 根の無い相対の先頭の `..` は残す (根より上へは戻らない)
                    out.push(c);
                }
            }
            other => out.push(other),
        }
    }
    out.iter().map(|c| c.as_os_str()).collect()
}

/// `abs` が `dir` の下にあるか (D-075)。部品の単位で、大文字小文字を無視して比べる
/// (`C:\shots` は `C:\shots2\a.png` を含まない)。「フォルダの下にあるか」はここだけで書く。
pub fn path_within(abs: &Path, dir: &Path) -> bool {
    let lower = |c: Component| c.as_os_str().to_string_lossy().to_lowercase();
    let mut a = abs.components();
    dir.components()
        .all(|d| a.next().is_some_and(|x| lower(x) == lower(d)))
}

/// 先頭 8 バイトが PNG の署名か (8 バイト未満なら偽)。
fn has_png_signature(r: &mut impl Read) -> io::Result<bool> {
    let mut head = Vec::with_capacity(8);
    r.take(8).read_to_end(&mut head)?;
    Ok(head == PNG_SIGNATURE)
}

fn not_png_message(path: &Path) -> String {
    format!(
        "撮影の保存先 {} に PNG ではないファイル (またはフォルダ) があるので上書きしない",
        path.display()
    )
}

/// 既にあるものを PNG で上書きしてよいか (D-075)。無ければ Ok。フォルダ、または先頭が PNG の署名でない
/// ファイル (8 バイト未満を含む) なら断る。メタデータや中身が読めないときも断る (理由を添える)。
pub fn check_overwritable(path: &Path) -> Result<(), String> {
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(format!("{} ({e})", not_png_message(path))),
    };
    if !meta.is_file() {
        return Err(not_png_message(path));
    }
    let png = std::fs::File::open(path)
        .and_then(|mut f| has_png_signature(&mut f))
        .map_err(|e| format!("{} ({e})", not_png_message(path)))?;
    if png {
        Ok(())
    } else {
        Err(not_png_message(path))
    }
}

/// ファイルに保存する。親のフォルダが無ければ作る。
/// 書く直前にパスの形と既存のファイルを確かめ (D-075)、確認と書き込みを同じハンドルで行う
/// (確認の後に差し替えられても潰さない)。断ったときの中身は元のまま。
pub struct PngFileWriter;

impl ImageWriter for PngFileWriter {
    fn write_png(&mut self, path: &str, image: &Image) -> Result<String, String> {
        check_save_path_shape(path)?;
        let bytes = encode_png(image)?;
        let p = Path::new(path);
        if let Some(dir) = p.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let mut file = match OpenOptions::new().write(true).create_new(true).open(p) {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                // 切り詰めずに開き、同じハンドルで確かめてから書く
                let mut f = OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(p)
                    .map_err(|e| {
                        if p.is_dir() {
                            not_png_message(p)
                        } else {
                            format!("{path}: {e}")
                        }
                    })?;
                let is_file = f.metadata().map_err(|e| format!("{path}: {e}"))?.is_file();
                let png =
                    is_file && has_png_signature(&mut f).map_err(|e| format!("{path}: {e}"))?;
                if !png {
                    return Err(not_png_message(p));
                }
                f.set_len(0).map_err(|e| format!("{path}: {e}"))?;
                f.seek(SeekFrom::Start(0))
                    .map_err(|e| format!("{path}: {e}"))?;
                f
            }
            Err(e) => {
                if p.is_dir() {
                    return Err(not_png_message(p));
                }
                return Err(format!("{path}: {e}"));
            }
        };
        file.write_all(&bytes).map_err(|e| format!("{path}: {e}"))?;
        Ok(std::path::absolute(p)
            .unwrap_or_else(|_| p.to_path_buf())
            .display()
            .to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_roundtrip() {
        let img = Image {
            width: 3,
            height: 2,
            rgba: (0..24).collect(),
        };
        let bytes = encode_png(&img).unwrap();
        assert_eq!(&bytes[1..4], b"PNG");
        assert_eq!(decode_png(&bytes).unwrap(), img);
    }

    /// 縮尺 (D-053): 箱型の平均で縮める。0.5 なら 2x2 の平均 (四捨五入)。1 なら変えない。最小は 1x1
    #[test]
    fn downscale_averages_boxes() {
        let black = [0u8, 0, 0, 255];
        let white = [255u8, 255, 255, 255];
        let mut rgba = Vec::new();
        for px in [black, black, white, white, white, white, black, black] {
            rgba.extend_from_slice(&px);
        }
        let img = Image {
            width: 4,
            height: 2,
            rgba,
        };
        let half = downscale(&img, 0.5);
        assert_eq!((half.width, half.height), (2, 1));
        assert_eq!(half.rgba, vec![128, 128, 128, 255, 128, 128, 128, 255]);
        assert_eq!(downscale(&img, 1.0), img, "1 なら変えない");
        let tiny = downscale(&img, 0.1);
        assert_eq!((tiny.width, tiny.height), (1, 1));
        assert_eq!(tiny.rgba, vec![128, 128, 128, 255]);
    }

    /// 長辺の上限 (D-072): 縮尺は scale と max_edge / 長辺 の小さい方。縮める向きだけ
    #[test]
    fn effective_scale_takes_smaller_of_scale_and_edge_limit() {
        assert_eq!(effective_scale(1.0, None, 40, 20), 1.0, "上限なし");
        assert_eq!(effective_scale(1.0, Some(40), 40, 20), 1.0, "長辺と同じ");
        assert_eq!(effective_scale(0.5, Some(100), 40, 20), 0.5, "長辺以下");
        assert_eq!(
            effective_scale(1.0, Some(1568), 3840, 2160),
            1568.0 / 3840.0,
            "横長"
        );
        assert_eq!(effective_scale(1.0, Some(10), 20, 40), 0.25, "縦長");
        assert_eq!(
            effective_scale(1.0, Some(10), 40, 20),
            0.25,
            "横長の小さい矩形"
        );
        assert_eq!(
            effective_scale(0.1, Some(10), 40, 20),
            0.1,
            "scale の方が小さい"
        );
        assert_eq!(effective_scale(0.7, Some(10), 0, 0), 0.7, "長辺 0");
    }

    /// `Some(0)` は形の確認で弾く前提。届いたら debug では止める (D-072)
    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "max_edge は 1 以上")]
    fn effective_scale_rejects_zero_edge_in_debug() {
        effective_scale(1.0, Some(0), 40, 20);
    }

    fn flat(w: u32, h: u32, v: u8) -> Image {
        Image {
            width: w,
            height: h,
            rgba: vec![v; (w * h * 4) as usize],
        }
    }

    /// 画面の待ち (D-081): RGB のどれかの差が許容差を超えた点の割合。ちょうど許容差なら数えない。大きさが違えば 1.0
    #[test]
    fn frame_diff_counts_pixels_beyond_tolerance() {
        let a = flat(2, 2, 100);
        assert_eq!(frame_diff(&a, &a.clone()), 0.0, "同じ画像");
        let mut b = a.clone();
        b.rgba[0] = 100 + 17;
        assert_eq!(frame_diff(&a, &b), 0.25, "4 点のうち 1 点の R が 17 違う");
        let mut c = a.clone();
        c.rgba[0] = 100 + 16;
        assert_eq!(frame_diff(&a, &c), 0.0, "16 は許容差以内");
        let mut d = a.clone();
        d.rgba[3] = 0;
        assert_eq!(frame_diff(&a, &d), 0.0, "アルファは見ない");
        let mut g = a.clone();
        g.rgba[4 + 1] = 100 + 17;
        assert_eq!(frame_diff(&a, &g), 0.25, "G だけが 17 違う点も数える");
        let mut bl = a.clone();
        bl.rgba[8 + 2] = 100 - 17;
        bl.rgba[12 + 2] = 100 + 17;
        assert_eq!(frame_diff(&a, &bl), 0.5, "B だけが 17 違う点も数える");
        assert_eq!(frame_diff(&a, &flat(2, 1, 100)), 1.0, "大きさが違う");
        assert_eq!(frame_diff(&flat(0, 0, 0), &flat(0, 0, 0)), 0.0, "空");
    }

    /// 比較用の縮小 (D-081): 長辺を DIFF_EDGE (256) 以下にする。小さければそのまま
    #[test]
    fn thumb_limits_long_edge_to_diff_edge() {
        let big = thumb(&flat(1000, 500, 7));
        assert_eq!((big.width, big.height), (256, 128));
        let small = flat(100, 50, 7);
        assert_eq!(thumb(&small), small);
    }

    /// 満たしたかの境界 (D-081): quiet は閾値ちょうどで真、changed は閾値ちょうどで偽、超えたら真
    #[test]
    fn quiet_and_changed_boundaries() {
        assert!(is_quiet(0.1, 0.1));
        assert!(!is_changed(0.1, 0.1));
        assert!(is_changed(0.1001, 0.1));
        assert!(!is_quiet(0.1001, 0.1));
        assert!(is_quiet(0.0, 0.0));
        assert!(!is_changed(0.0, 0.0));
    }

    /// 保存先の形 (D-075): .png だけ。代替データストリームと \\?\ / \\.\ は断る
    #[test]
    fn save_path_shape_accepts_png_only() {
        for ok in ["a.png", "A.PNG", "dir/x.Png", "C:\\a\\b.png"] {
            assert!(check_save_path_shape(ok).is_ok(), "{ok}");
        }
        for bad in ["a.txt", "a", "mcp:inline", "a.png.txt"] {
            assert!(check_save_path_shape(bad).is_err(), "{bad}");
        }
        for ads in ["a.png:x", "C:\\docs\\report.docx:a.png", "dir\\x:y.png"] {
            let e = check_save_path_shape(ads).unwrap_err();
            assert!(e.contains("代替データストリーム"), "{ads}: {e}");
        }
        for dev in ["\\\\?\\C:\\a.png", "//./C:/a.png"] {
            let e = check_save_path_shape(dev).unwrap_err();
            assert!(e.contains("\\\\?\\"), "{dev}: {e}");
        }
        assert!(is_verbatim_or_device("//?/C:/s"));
        assert!(!is_verbatim_or_device("\\\\server\\share\\a.png"));
    }

    /// 字面の正規化 (D-075): cwd に足して . と .. を畳む。根より上には戻らない。ドライブ相対は絶対にならない
    #[test]
    fn normalize_save_path_folds_dots_against_cwd() {
        let cwd = Path::new("C:\\work\\proj");
        assert_eq!(
            normalize_save_path("target\\..\\..\\x.png", cwd),
            PathBuf::from("C:\\work\\x.png")
        );
        assert_eq!(
            normalize_save_path(".\\a.png", cwd),
            PathBuf::from("C:\\work\\proj\\a.png")
        );
        assert_eq!(
            normalize_save_path("D:\\a\\.\\b\\..\\c.png", cwd),
            PathBuf::from("D:\\a\\c.png")
        );
        assert_eq!(
            normalize_save_path("C:\\..\\a.png", cwd),
            PathBuf::from("C:\\a.png")
        );
        assert!(!normalize_save_path("D:x.png", cwd).is_absolute());
    }

    /// フォルダの包含 (D-075): 部品の単位で大文字小文字を無視する
    #[test]
    fn path_within_compares_components_case_insensitively() {
        let dir = Path::new("C:\\Shots");
        assert!(path_within(Path::new("c:\\shots\\a.png"), dir));
        assert!(path_within(Path::new("C:\\shots\\sub\\a.png"), dir));
        for out in ["C:\\shots2\\a.png", "C:\\a.png", "D:\\shots\\a.png"] {
            assert!(!path_within(Path::new(out), dir), "{out}");
        }
    }

    fn tiny_png() -> Vec<u8> {
        encode_png(&Image {
            width: 1,
            height: 1,
            rgba: vec![1, 2, 3, 255],
        })
        .unwrap()
    }

    /// 既存のもの (D-075): PNG と存在しないパスだけ上書きしてよい。テキスト、0 バイト、フォルダは断る
    #[test]
    fn check_overwritable_refuses_non_png_and_dirs() {
        let dir = crate::testing::unique_temp_dir("overwritable");
        let png = dir.join("p.png");
        std::fs::write(&png, tiny_png()).unwrap();
        let text = dir.join("t.png");
        std::fs::write(&text, "hello").unwrap();
        let empty = dir.join("e.png");
        std::fs::write(&empty, b"").unwrap();
        let sub = dir.join("d.png");
        std::fs::create_dir_all(&sub).unwrap();
        assert!(check_overwritable(&png).is_ok());
        assert!(check_overwritable(&dir.join("none.png")).is_ok());
        for bad in [&text, &empty, &sub] {
            let e = check_overwritable(bad).unwrap_err();
            assert!(e.contains("PNG ではない"), "{}: {e}", bad.display());
        }
    }

    /// 書く直前の守り (D-075): PNG でない既存ファイルは中身を残して断る。.png でないパスはファイルを作らない
    #[test]
    fn png_file_writer_refuses_non_png_target_and_keeps_its_bytes() {
        let dir = crate::testing::unique_temp_dir("png-writer");
        let img = Image {
            width: 1,
            height: 1,
            rgba: vec![9, 9, 9, 255],
        };
        let text = dir.join("notes.png");
        std::fs::write(&text, "my notes").unwrap();
        let e = PngFileWriter
            .write_png(text.to_str().unwrap(), &img)
            .unwrap_err();
        assert!(e.contains("PNG ではない"), "{e}");
        assert_eq!(std::fs::read(&text).unwrap(), b"my notes");

        // 名前が .png のフォルダは、io の誤りではなく「PNG ではない」の文で断り、フォルダのまま残す
        let sub = dir.join("d.png");
        std::fs::create_dir(&sub).unwrap();
        let e = PngFileWriter
            .write_png(sub.to_str().unwrap(), &img)
            .unwrap_err();
        assert!(e.contains("PNG ではない"), "{e}");
        assert!(sub.is_dir());

        // 0 バイトの .png は署名が無いので断り、0 バイトのまま残す
        let empty = dir.join("empty.png");
        std::fs::write(&empty, b"").unwrap();
        let e = PngFileWriter
            .write_png(empty.to_str().unwrap(), &img)
            .unwrap_err();
        assert!(e.contains("PNG ではない"), "{e}");
        assert!(std::fs::read(&empty).unwrap().is_empty());

        let txt = dir.join("x.txt");
        assert!(
            PngFileWriter
                .write_png(txt.to_str().unwrap(), &img)
                .is_err()
        );
        assert!(!txt.exists());

        // PNG は今までどおり上書きでき、新しいパスには書ける
        let png = dir.join("old.png");
        std::fs::write(&png, tiny_png()).unwrap();
        PngFileWriter
            .write_png(png.to_str().unwrap(), &img)
            .unwrap();
        assert_eq!(decode_png(&std::fs::read(&png).unwrap()).unwrap(), img);
        let fresh = dir.join("new").join("n.png");
        PngFileWriter
            .write_png(fresh.to_str().unwrap(), &img)
            .unwrap();
        assert_eq!(decode_png(&std::fs::read(&fresh).unwrap()).unwrap(), img);
    }

    #[test]
    fn png_rejects_wrong_buffer_size() {
        assert!(
            encode_png(&Image {
                width: 3,
                height: 2,
                rgba: vec![0; 5]
            })
            .is_err()
        );
    }
}
