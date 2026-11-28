//! Shell-owned drag image. No window, thumbnail decoding or file enumeration.
#![allow(unsafe_code)]
use windows::core::Interface;
use windows::Win32::{
    Foundation::{COLORREF, HWND, POINT, SIZE},
    Graphics::Gdi::*,
    System::Com::IDataObject,
    UI::{Shell::*, WindowsAndMessaging::{DestroyIcon, DrawIconEx, DI_NORMAL}},
};

thread_local! { static FOLDER_HINT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }
pub(crate) fn set_folder_hint(folder: bool) { FOLDER_HINT.with(|hint| hint.set(folder)); }

/// Keep both the source helper and its bitmap alive across the nested OLE loop.
pub(super) struct DragImageSession {
    _helper: IDragSourceHelper,
    _bitmap: DragBitmap,
}

pub(super) struct DragBitmap(HBITMAP);
impl Drop for DragBitmap {
    fn drop(&mut self) { unsafe { let _ = DeleteObject(HGDIOBJ(self.0.0)); } }
}

pub(super) fn begin(data: &IDataObject, hwnd: HWND, count: usize) -> windows::core::Result<DragImageSession> {
    unsafe {
        let helper: IDragSourceHelper = windows::Win32::System::Com::CoCreateInstance(
            &CLSID_DragDropHelper, None, windows::Win32::System::Com::CLSCTX_INPROC_SERVER)?;
        if let Ok(extended) = helper.cast::<IDragSourceHelper2>() {
            extended.SetFlags(DSH_ALLOWDROPDESCRIPTIONTEXT.0 as u32)?;
        }
        let bitmap = install(&helper, data, hwnd, count)?;
        Ok(DragImageSession { _helper: helper, _bitmap: bitmap })
    }
}

fn install(helper: &IDragSourceHelper, data: &IDataObject, hwnd: HWND, count: usize) -> windows::core::Result<DragBitmap> {
    unsafe {
        let dc = CreateCompatibleDC(None);
        if dc.0.is_null() { return Err(windows::core::Error::from_hresult(windows::Win32::Foundation::E_FAIL)); }
        let scale = (windows::Win32::UI::HiDpi::GetDpiForWindow(hwnd) as f32 / 96.0).max(1.0);
        let width = ((112.0f32).max(72.0 + count.to_string().len() as f32 * 8.0) * scale).round() as i32;
        let height = (64.0 * scale).round() as i32;
        let mut info = BITMAPINFO::default();
        info.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        info.bmiHeader.biWidth = width;
        info.bmiHeader.biHeight = -height;
        info.bmiHeader.biPlanes = 1;
        info.bmiHeader.biBitCount = 32;
        info.bmiHeader.biCompression = BI_RGB.0;
        let mut bits = std::ptr::null_mut();
        let bitmap = match CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0) {
            Ok(bitmap) => bitmap,
            Err(error) => { let _ = DeleteDC(dc); return Err(error); }
        };
        let old = SelectObject(dc, HGDIOBJ(bitmap.0));
        let pixels = std::slice::from_raw_parts_mut(bits.cast::<u32>(), (width * height) as usize);
        pixels.fill(0);
        // Small offset cards make a multi-selection recognizable even before reading the count.
        let mut fill = |x: i32, y: i32, w: i32, h: i32, color: u32| {
            for py in y.max(0)..(y + h).min(height) {
                for px in x.max(0)..(x + w).min(width) { pixels[(py * width + px) as usize] = color; }
            }
        };
        if count > 1 { fill((14. * scale) as i32, (12. * scale) as i32, (44. * scale) as i32, (44. * scale) as i32, 0xff8b99ad); }
        fill((8. * scale) as i32, (6. * scale) as i32, (44. * scale) as i32, (44. * scale) as i32, 0xffedf2fa);
        let mut icon = SHSTOCKICONINFO { cbSize: std::mem::size_of::<SHSTOCKICONINFO>() as u32, ..Default::default() };
        let kind = if FOLDER_HINT.with(|hint| hint.get()) { SIID_FOLDER } else { SIID_DOCNOASSOC };
        if SHGetStockIconInfo(kind, SHGSI_ICON | SHGSI_LARGEICON, &mut icon).is_ok() {
            let _ = DrawIconEx(dc, (14. * scale) as i32, (12. * scale) as i32, icon.hIcon,
                (32. * scale) as i32, (32. * scale) as i32, 0, None, DI_NORMAL);
            let _ = GdiFlush();
            let _ = DestroyIcon(icon.hIcon);
        }
        if count > 1 {
            // Tiny integer glyphs avoid creating a font or rasterizing an image at drag start.
            let digits = [0b111101101101111u16, 0b010110010010111, 0b111001111100111,
                0b111001111001111, 0b101101111001001, 0b111100111001111,
                0b111100111101111, 0b111001001001001, 0b111101111101111, 0b111101111001111];
            let label = count.to_string();
            let unit = (2.0 * scale).round().max(1.0) as i32;
            let bx = (60. * scale) as i32;
            let by = (24. * scale) as i32;
            for y in by - unit..by + 6 * unit { for x in bx - unit..bx + (label.len() as i32 * 4) * unit {
                if x < width && y < height { pixels[(y * width + x) as usize] = 0xff214d82; }
            }}
            for (index, digit) in label.bytes().enumerate() {
                let mask = digits[(digit - b'0') as usize];
                for y in 0..5 { for x in 0..3 {
                    if mask & (1 << (14 - y * 3 - x)) != 0 {
                        for dy in 0..unit { for dx in 0..unit {
                            let px = bx + (index as i32 * 4 + x) * unit + dx;
                            let py = by + y * unit + dy;
                            if px < width && py < height { pixels[(py * width + px) as usize] = 0xffffffff; }
                        }}
                    }
                }}
            }
        }
        let _ = GdiFlush();
        // The Shell premultiplies itself. Supply straight BGRA, avoiding dark fringes.
        for pixel in pixels.iter_mut().filter(|pixel| **pixel != 0) {
            let source_alpha = (*pixel >> 24).max(1);
            let channel = |shift: u32| -> u32 {
                let value = (*pixel >> shift) & 255;
                if source_alpha < 255 { (value * 255 / source_alpha).min(255) } else { value }
            };
            let alpha = if source_alpha == 1 { 210 } else { source_alpha * 210 / 255 };
            *pixel = (alpha << 24) | (channel(16) << 16) | (channel(8) << 8) | channel(0);
        }
        let _ = SelectObject(dc, old);
        let _ = DeleteDC(dc);
        let image = SHDRAGIMAGE { sizeDragImage: SIZE { cx: width, cy: height },
            ptOffset: POINT { x: (12. * scale) as i32, y: (12. * scale) as i32 },
            hbmpDragImage: bitmap, crColorKey: COLORREF(u32::MAX) };
        let bitmap = DragBitmap(bitmap);
        helper.InitializeFromBitmap(&image, data)?;
        Ok(bitmap)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{path::PathBuf, sync::{Arc, atomic::AtomicU32}};
    use windows::Win32::System::{Com::{DATADIR_GET, DVASPECT_CONTENT, FORMATETC, TYMED_HGLOBAL},
        Ole::{OleInitialize, OleUninitialize, ReleaseStgMedium, DROPEFFECT_COPY}, Memory::GlobalSize};

    #[test]
    fn shell_initialization_publishes_readable_image_data_for_single_and_multiple_items() {
        unsafe {
            OleInitialize(None).unwrap();
            struct OleGuard;
            impl Drop for OleGuard { fn drop(&mut self) { unsafe { OleUninitialize(); } } }
            let _ole = OleGuard;
            use windows::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, WS_POPUP};
            let hwnd = CreateWindowExW(Default::default(), windows::core::w!("STATIC"),
                windows::core::w!("FileOP native drag lifecycle test"), WS_POPUP,
                0, 0, 200, 120, None, None, None, None).unwrap();
            struct WindowGuard(HWND);
            impl Drop for WindowGuard { fn drop(&mut self) { unsafe { let _ = DestroyWindow(self.0); } } }
            let _window = WindowGuard(hwnd);
            for (count, folder) in [(1, false), (1, true), (3, false)] {
                set_folder_hint(folder);
                let paths = vec![PathBuf::from(r"C:\FileOP-preview-test.txt"); count];
                let data = super::super::data_object::AtlasDataObject::new(&paths, DROPEFFECT_COPY,
                    Arc::new(AtomicU32::new(0))).unwrap();
                let session = begin(&data, hwnd, count).unwrap();
                let formats = data.EnumFormatEtc(DATADIR_GET.0 as u32).unwrap();
                let mut found = false;
                loop {
                    let mut fmt = FORMATETC::default();
                    let mut fetched = 0;
                    let result = formats.Next(std::slice::from_mut(&mut fmt), Some(&mut fetched));
                    if result.is_err() || fetched == 0 { break; }
                    let mut name = [0u16; 128];
                    let len = windows::Win32::System::DataExchange::GetClipboardFormatNameW(fmt.cfFormat as u32, &mut name);
                    if String::from_utf16_lossy(&name[..len.max(0) as usize]) == "DragImageBits" {
                        fmt.dwAspect = DVASPECT_CONTENT.0;
                        fmt.tymed = TYMED_HGLOBAL.0 as u32;
                        data.QueryGetData(&fmt).ok().unwrap();
                        let mut image = data.GetData(&fmt).unwrap();
                        assert!(GlobalSize(image.u.hGlobal) > 64);
                        ReleaseStgMedium(&mut image);
                        found = true;
                    }
                }
                assert!(found, "Shell initialization did not publish DragImageBits");
                let target: IDropTargetHelper = windows::Win32::System::Com::CoCreateInstance(
                    &CLSID_DragDropHelper, None, windows::Win32::System::Com::CLSCTX_INPROC_SERVER).unwrap();
                // Exercise native target lifecycle on an unshown test window. This
                // validates COM calls and ownership, not human-visible cursor motion.
                target.Show(false).unwrap();
                let point = POINT { x: 20, y: 20 };
                target.DragEnter(hwnd, &data, &point, DROPEFFECT_COPY).unwrap();
                target.DragOver(&point, DROPEFFECT_COPY).unwrap();
                target.DragLeave().unwrap();
                target.DragEnter(hwnd, &data, &point, DROPEFFECT_COPY).unwrap();
                target.Drop(&data, &point, DROPEFFECT_COPY).unwrap();
                drop(target);
                drop(session);
            }
        }
    }
}
