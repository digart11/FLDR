//! Raster thumbnail decoding; caller retains its existing orientation policy.
use std::path::Path;
use image::ImageDecoder as _;

pub fn raster_thumbnail(path: &Path, edge: u32, orient: bool) -> image::ImageResult<(Vec<u8>, usize, usize)> {
    if super::is_cloud_placeholder(path) {
        return Err(std::io::Error::new(std::io::ErrorKind::WouldBlock, "Cloud placeholder").into());
    }
    let reader = image::ImageReader::open(path)?;
    let reader = if orient { reader.with_guessed_format()? } else { reader };
    #[cfg(windows)]
    if reader.format() == Some(image::ImageFormat::Jpeg) {
        use std::io::Read as _;
        // Eligibility cap, not a new rejection limit: larger compressed files
        // retain the established decoder. The bytes also bind metadata and WIC
        // pixels to the same input if a file changes while a job is running.
        const MAX_COMPRESSED: usize = 32 * 1024 * 1024;
        let mut source = reader.into_inner();
        let mut bytes = Vec::new();
        source.by_ref().take(MAX_COMPRESSED as u64 + 1).read_to_end(&mut bytes)?;
        if bytes.len() <= MAX_COMPRESSED {
            if let Some(thumbnail) = reduced_thumbnail(&bytes, edge, orient)? { return Ok(thumbnail); }
            // Move the captured input into the old decoder. Its reader buffer
            // drops during decoder construction, before full-size pixels exist.
            // This avoids both another path open and retaining probe buffers.
            return full_thumbnail(image::ImageReader::with_format(std::io::Cursor::new(bytes), image::ImageFormat::Jpeg), edge, orient);
        }
        drop(bytes);
        std::io::Seek::rewind(&mut source)?;
        return full_thumbnail(image::ImageReader::with_format(source, image::ImageFormat::Jpeg), edge, orient);
    }
    full_thumbnail(reader, edge, orient)
}

#[cfg(windows)]
fn reduced_thumbnail(bytes: &[u8], edge: u32, orient: bool) -> image::ImageResult<Option<(Vec<u8>, usize, usize)>> {
    let mut decoder = image::ImageReader::with_format(std::io::Cursor::new(bytes), image::ImageFormat::Jpeg).into_decoder()?;
    let (width, height) = decoder.dimensions();
    if edge == 0 || width.max(height) <= edge.saturating_mul(2)
        // Odd-sized cropped JPEGs showed excessive differences on fine text.
        // Restrict the first optimization to complete 8-pixel DCT dimensions.
        || width % 8 != 0 || height % 8 != 0
        || !matches!(decoder.color_type(), image::ColorType::Rgb8 | image::ColorType::L8)
        || image::Limits::default().max_alloc.is_some_and(|limit| decoder.total_bytes() > limit)
        || !decoder.icc_profile().is_ok_and(|profile| profile.is_none()) { return Ok(None); }
    let orientation = if orient { decoder.orientation().unwrap_or(image::metadata::Orientation::NoTransforms) }
        else { image::metadata::Orientation::NoTransforms };
    let scaled = atlas_win_space::jpeg::decode_reduced(bytes, width, height, edge)
        .and_then(|reduced| if reduced.grayscale {
            image::GrayImage::from_raw(reduced.width, reduced.height, reduced.pixels).map(image::DynamicImage::ImageLuma8)
        } else {
            image::RgbImage::from_raw(reduced.width, reduced.height, reduced.pixels).map(image::DynamicImage::ImageRgb8)
        });
    let Some(mut image) = scaled else { return Ok(None); };
    image.apply_orientation(orientation);
    let ratio = f64::from(edge) / f64::from(width.max(height));
    let (mut w, mut h) = (((f64::from(width) * ratio).round() as u32).max(1), ((f64::from(height) * ratio).round() as u32).max(1));
    use image::metadata::Orientation::*;
    if matches!(orientation, Rotate90 | Rotate270 | Rotate90FlipH | Rotate270FlipH) { std::mem::swap(&mut w, &mut h); }
    Ok(Some(rgba(image.thumbnail_exact(w, h))))
}

fn full_thumbnail<R: std::io::BufRead + std::io::Seek>(reader: image::ImageReader<R>, edge: u32, orient: bool) -> image::ImageResult<(Vec<u8>, usize, usize)> {
    let image = if orient {
        let mut decoder = reader.into_decoder()?;
        let orientation = decoder.orientation().unwrap_or(image::metadata::Orientation::NoTransforms);
        let mut image = image::DynamicImage::from_decoder(decoder)?;
        image.apply_orientation(orientation);
        image
    } else { reader.decode()? };
    Ok(rgba(image.thumbnail(edge, edge)))
}

fn rgba(image: image::DynamicImage) -> (Vec<u8>, usize, usize) {
    let rgba = image.to_rgba8();
    let (w, h) = rgba.dimensions();
    (rgba.into_raw(), w as usize, h as usize)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn jpeg(width: u32, height: u32, gray: bool) -> Vec<u8> {
        let rgb = image::RgbImage::from_fn(width, height, |x,y| {
            let base = match (x < width/2, y < height/2) { (true,true) => [220,30,40], (false,true) => [30,220,40], (true,false) => [30,40,220], _ => [220,180,40] };
            image::Rgb(base)
        });
        let image = if gray { image::DynamicImage::ImageLuma8(image::DynamicImage::ImageRgb8(rgb).to_luma8()) } else { image::DynamicImage::ImageRgb8(rgb) };
        let mut bytes = Vec::new();
        if gray {
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes,95).encode(image.to_luma8().as_raw(),width,height,image::ExtendedColorType::L8).unwrap();
        } else { image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes,95).encode_image(&image).unwrap(); }
        bytes
    }
    fn reference(path: &Path, edge: u32, orient: bool) -> image::ImageResult<(Vec<u8>,usize,usize)> {
        let image = if orient {
            let mut decoder = image::ImageReader::open(path)?.with_guessed_format()?.into_decoder()?;
            let orientation = decoder.orientation().unwrap_or(image::metadata::Orientation::NoTransforms);
            let mut image = image::DynamicImage::from_decoder(decoder)?;
            image.apply_orientation(orientation); image
        } else { image::open(path)? };
        let rgba = image.thumbnail(edge,edge).to_rgba8();
        let (w,h) = rgba.dimensions(); Ok((rgba.into_raw(),w as usize,h as usize))
    }
    fn app_segment(bytes: &[u8], marker: u8, body: &[u8]) -> Vec<u8> {
        let mut out = vec![0xff,0xd8,0xff,marker];
        out.extend_from_slice(&((body.len()+2) as u16).to_be_bytes());
        out.extend_from_slice(body);out.extend_from_slice(&bytes[2..]);out
    }
    #[test]
    fn jpeg_thumbnails_preserve_all_exif_orientations_dimensions_and_rgb() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = jpeg(1024,768,false);
        #[cfg(windows)]
        assert!(atlas_win_space::jpeg::decode_reduced(&bytes,1024,768,128).is_some(), "Native reduction must actually be exercised");
        for orientation in 1..=8u8 {
            let mut exif = b"Exif\0\0II\x2a\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0".to_vec();
            exif.extend_from_slice(&[orientation,0,0,0,0,0,0,0]);
            let path = dir.path().join(format!("o{orientation}.jpg"));
            std::fs::write(&path,app_segment(&bytes,0xe1,&exif)).unwrap();
            for orient in [false,true] { for edge in [32,128,256,384] {
                let (actual,w,h) = raster_thumbnail(&path,edge,orient).unwrap();
                let (expected,ew,eh) = reference(&path,edge,orient).unwrap();
                assert_eq!((w,h),(ew,eh)); assert_eq!(actual.len(),expected.len());
                assert!(actual.chunks_exact(4).all(|p|p[3]==255));
                let error: u64 = actual.iter().zip(&expected).map(|(a,b)|u64::from(a.abs_diff(*b))).sum();
                assert!((error as f64 / actual.len() as f64) < 4., "orientation={orientation}, edge={edge}, error={error}");
            }}
        }
    }
    #[test]
    fn small_profiled_gray_nonjpeg_and_invalid_images_keep_fallbacks() {
        let dir = tempfile::tempdir().unwrap();
        for (name,bytes) in [("small.jpg",jpeg(40,20,false)),("gray.jpg",jpeg(1024,768,true)),("odd.jpg",jpeg(1025,769,false))] {
            let path=dir.path().join(name);std::fs::write(&path,&bytes).unwrap();
            for orient in [false,true] {
                let (actual,w,h)=raster_thumbnail(&path,128,orient).unwrap();
                let (expected,ew,eh)=reference(&path,128,orient).unwrap();assert_eq!((w,h),(ew,eh));
                if name!="gray.jpg" {assert_eq!(actual,expected);} else {
                    assert!(actual.chunks_exact(4).all(|p|p[0]==p[1] && p[1]==p[2] && p[3]==255));
                }
            }
        }
        let bytes=jpeg(1024,768,false);
        let profiled=app_segment(&bytes,0xe2,b"ICC_PROFILE\0\x01\x01opaque-profile-bytes");
        let path=dir.path().join("profile.jpg");std::fs::write(&path,&profiled).unwrap();
        let mut decoder=image::codecs::jpeg::JpegDecoder::new(std::io::Cursor::new(&profiled)).unwrap();
        assert!(decoder.icc_profile().unwrap().is_some());
        assert_eq!(raster_thumbnail(&path,128,true).unwrap(),reference(&path,128,true).unwrap());
        let png=dir.path().join("rgba.png");image::RgbaImage::from_pixel(31,17,image::Rgba([200,80,20,70])).save(&png).unwrap();
        for orient in [false,true] { assert_eq!(raster_thumbnail(&png,64,orient).unwrap(),reference(&png,64,orient).unwrap()); }
        for length in [0,2,16,bytes.len()/2,bytes.len()-16] {
            let path=dir.path().join("broken.jpg");std::fs::write(&path,&bytes[..length]).unwrap();
            assert_eq!(raster_thumbnail(&path,128,true).is_ok(),reference(&path,128,true).is_ok());
        }
        assert!(raster_thumbnail(&dir.path().join("missing.jpg"),128,true).is_err());
    }

    #[tokio::test]
    async fn multi_selection_pipeline_keeps_identity_orientation_and_failure_reporting() {
        use atlas_model::{app_state::BgResult, layout_tree::TileId, preview_cache::MultiPreviewKey};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("portrait.jpg");
        let mut exif = b"Exif\0\0II\x2a\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0".to_vec();
        exif.extend_from_slice(&[6,0,0,0,0,0,0,0]);
        std::fs::write(&path,app_segment(&jpeg(1024,768,false),0xe1,&exif)).unwrap();
        let key = MultiPreviewKey { fingerprint: 123, selected_count: 2 };
        let (tx,rx) = flume::unbounded();
        crate::fs::preview::load_multi_thumbnail(TileId(7),key.clone(),0,path.clone(),tx).await;
        let BgResult::MultiPreviewThumbnailReady { tab, key: actual_key, path: actual_path, rgba, width, height } = rx.recv().unwrap() else { panic!("thumbnail result"); };
        assert_eq!(tab,TileId(7)); assert_eq!(actual_key,key); assert_eq!(actual_path,path);
        assert_eq!((width,height),(288,384)); assert_eq!(rgba.len(),width*height*4);
        let (tx,rx) = flume::unbounded();
        crate::fs::preview::load_multi_thumbnail(TileId(7),key,0,dir.path().join("missing.jpg"),tx).await;
        assert!(rx.try_recv().is_err(), "A failed decode must not send a successful thumbnail");
    }

    #[test]
    fn compressed_probe_cap_falls_back_without_rejecting_the_image() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("padded.jpg");
        let mut bytes = jpeg(40,24,false);
        bytes.resize(32*1024*1024+2,0);
        std::fs::write(&path,bytes).unwrap();
        assert_eq!(raster_thumbnail(&path,128,false).unwrap(),reference(&path,128,false).unwrap());
    }
}
