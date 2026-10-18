// Пиксели иконки: мини-пианоролл на графите с полосой «опасность» снизу.
// Подключается и в приложение (иконка окна), и в build.rs (иконка exe).

pub fn icon_rgba(size: u32) -> Vec<u8> {
    let mut px = vec![0u8; (size * size * 4) as usize];
    let s = size as f32;
    let ink = [14u8, 15, 17, 255];
    let yellow = [255u8, 210, 31, 255];
    let paper = [232u8, 228, 218, 255];
    let stripe = (size / 8).max(1);
    for y in 0..size {
        for x in 0..size {
            let fy = y as f32 / s;
            let c = if fy > 0.82 && ((x + y) / stripe).is_multiple_of(2) { yellow } else { ink };
            let i = ((y * size + x) * 4) as usize;
            px[i..i + 4].copy_from_slice(&c);
        }
    }
    let notes: [(f32, f32, f32, [u8; 4]); 3] = [(0.12, 0.18, 0.55, paper), (0.30, 0.38, 0.80, yellow), (0.55, 0.58, 0.88, paper)];
    for (y0, x0, x1, c) in notes {
        let (ya, yb) = ((y0 * s) as u32, (((y0 + 0.13) * s) as u32).max((y0 * s) as u32 + 1));
        for y in ya..yb.min(size) {
            for x in (x0 * s) as u32..((x1 * s) as u32).min(size) {
                let i = ((y * size + x) * 4) as usize;
                px[i..i + 4].copy_from_slice(&c);
            }
        }
    }
    px
}
