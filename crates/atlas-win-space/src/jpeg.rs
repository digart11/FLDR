//! Reduced decode through the inbox JPEG codec, never a Shell thumbnail provider.
use windows::{core::Interface, Win32::{Foundation::RPC_E_CHANGED_MODE, Graphics::Imaging::*, System::Com::*}};

pub struct ReducedJpeg {
    pub pixels: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub grayscale: bool,
}

struct Apartment(bool);
impl Drop for Apartment {
    fn drop(&mut self) { if self.0 { unsafe { CoUninitialize(); } } }
}

/// `bytes` stays alive until all COM objects release their memory-backed stream.
/// Unsupported modes, no actual reduction, or any native error return None.
pub fn decode_reduced(bytes: &[u8], width: u32, height: u32, edge: u32) -> Option<ReducedJpeg> {
    if edge == 0 || width.max(height) <= edge.saturating_mul(2) || width % 8 != 0 || height % 8 != 0
        || !bytes.starts_with(&[0xff, 0xd8]) || !bytes.ends_with(&[0xff, 0xd9]) { return None; }
    let initialized = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    if initialized.is_err() && initialized != RPC_E_CHANGED_MODE { return None; }
    let _apartment = Apartment(initialized.is_ok());
    let decode = || -> windows::core::Result<Option<ReducedJpeg>> { unsafe {
        let factory: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
        let stream = factory.CreateStream()?;
        stream.InitializeFromMemory(bytes)?;
        // Explicit inbox CLSID avoids choosing a third-party JPEG codec/provider.
        let decoder: IWICBitmapDecoder = CoCreateInstance(&CLSID_WICJpegDecoder, None, CLSCTX_INPROC_SERVER)?;
        decoder.Initialize(&stream, WICDecodeMetadataCacheOnDemand)?;
        let frame = decoder.GetFrame(0)?;
        let mut header = WICJpegFrameHeader::default();
        frame.cast::<IWICJpegFrameDecode>()?.GetFrameHeader(&mut header)?;
        if header.Width != width || header.Height != height
            || header.ScanType != WICJpegScanTypeInterleaved || !matches!(header.cComponents, 1 | 3) { return Ok(None); }
        let original_format = frame.GetPixelFormat()?;
        let grayscale = original_format == GUID_WICPixelFormat8bppGray;
        if !grayscale && original_format != GUID_WICPixelFormat24bppBGR && original_format != GUID_WICPixelFormat24bppRGB { return Ok(None); }
        let transform: IWICBitmapSourceTransform = frame.cast()?;
        if !transform.DoesSupportTransform(WICBitmapTransformRotate0)?.as_bool() { return Ok(None); }
        let ratio = f64::from(edge) / f64::from(width.max(height));
        let (mut w, mut h) = (((f64::from(width) * ratio).ceil() as u32).max(1), ((f64::from(height) * ratio).ceil() as u32).max(1));
        transform.GetClosestSize(&mut w, &mut h)?;
        if w >= width || h >= height || w == 0 || h == 0 || w.max(h) < edge { return Ok(None); }
        let mut format = if grayscale { GUID_WICPixelFormat8bppGray } else { GUID_WICPixelFormat24bppRGB };
        transform.GetClosestPixelFormat(&mut format)?;
        if grayscale && format != GUID_WICPixelFormat8bppGray { return Ok(None); }
        if !grayscale && format != GUID_WICPixelFormat24bppRGB && format != GUID_WICPixelFormat24bppBGR { return Ok(None); }
        let stride = w.checked_mul(if grayscale { 1 } else { 3 });
        let Some((stride, length)) = stride.and_then(|stride| stride.checked_mul(h).map(|length| (stride, length)))
            .filter(|(_, length)| *length <= 32 * 1024 * 1024) else { return Ok(None); };
        let mut pixels = vec![0; length as usize];
        transform.CopyPixels(std::ptr::null(), w, h, &format, WICBitmapTransformRotate0, stride, &mut pixels)?;
        if format == GUID_WICPixelFormat24bppBGR { for pixel in pixels.chunks_exact_mut(3) { pixel.swap(0, 2); } }
        Ok(Some(ReducedJpeg { pixels, width: w, height: h, grayscale }))
    }};
    decode().ok().flatten()
}
