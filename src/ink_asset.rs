//! Validated, independently compressed source frames; only adjacent frames are decoded.
use miniz_oxide::inflate::decompress_to_vec_zlib_with_limit;
const PACK: &[u8] = include_bytes!("../assets/ink/ink.frames");
pub struct Asset {
    pub width: u32,
    pub height: u32,
    pub count: u32,
    table: usize,
    body: usize,
}
fn word(offset: usize) -> Result<u32, String> {
    PACK.get(offset..offset + 4)
        .and_then(|b| b.try_into().ok())
        .map(u32::from_le_bytes)
        .ok_or_else(|| "水墨素材头损坏".into())
}
impl Asset {
    pub fn new() -> Result<Self, String> {
        if PACK.get(..4) != Some(b"INK2") {
            return Err("水墨素材格式不匹配".into());
        }
        let asset = Self {
            width: word(4)?,
            height: word(8)?,
            count: word(12)?,
            table: 20,
            body: 20 + (word(12)? as usize + 1) * 4,
        };
        if asset.width != 540
            || asset.height != 960
            || !(2..=1000).contains(&asset.count)
            || word(16)? != 24
            || asset.body > PACK.len()
            || asset.body + word(asset.table + asset.count as usize * 4)? as usize != PACK.len()
        {
            return Err("水墨素材尺寸/索引无效".into());
        }
        Ok(asset)
    }
    pub fn frame(&self, index: u32) -> Result<Vec<u8>, String> {
        if index >= self.count {
            return Err("水墨帧索引超出范围".into());
        }
        let start = word(self.table + index as usize * 4)? as usize;
        let end = word(self.table + (index as usize + 1) * 4)? as usize;
        let data = PACK
            .get(self.body + start..self.body + end)
            .ok_or("水墨帧索引损坏")?;
        let length = self.width as usize * self.height as usize * 2;
        let pixels = decompress_to_vec_zlib_with_limit(data, length)
            .map_err(|e| format!("水墨帧解码失败：{e:?}"))?;
        if pixels.len() != length {
            return Err("水墨帧长度错误".into());
        }
        Ok(pixels)
    }
}
pub fn smooth(p: f32) -> f32 {
    let p = p.clamp(0., 1.);
    p * p * (3. - 2. * p)
}
pub fn frame_pair(progress: f32, corner: usize, count: u32) -> (u32, u32, f32) {
    let delay = [0., 0.018, 0.033, 0.048][corner];
    let local = ((progress - delay) / (1. - delay)).clamp(0., 1.);
    let index = local * (count - 1) as f32;
    let a = index.floor() as u32;
    (a, (a + 1).min(count - 1), index - a as f32)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_continuous_frames_decode_and_finish_at_last_frame() {
        let asset = Asset::new().unwrap();
        assert!(asset.count >= 400);
        for i in 0..asset.count {
            assert_eq!(asset.frame(i).unwrap().len(), 540 * 960 * 2);
        }
        for corner in 0..4 {
            assert_eq!(frame_pair(0., corner, asset.count), (0, 1, 0.));
            let (a, b, t) = frame_pair(1., corner, asset.count);
            assert_eq!((a, b, t), (asset.count - 1, asset.count - 1, 0.));
            let mut previous = 0.;
            for i in 0..=1000 {
                let (a, b, t) = frame_pair(i as f32 / 1000., corner, asset.count);
                assert!(a < asset.count && b < asset.count && (0. ..=1.).contains(&t));
                let position = a as f32 + t;
                assert!(position >= previous);
                previous = position;
            }
        }
        assert!(asset.frame(asset.count).is_err());
    }
}
