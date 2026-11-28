//! `IDropTarget` implementation — Atlas drop target.
//!
//! Replaces gpui's built-in `WindowsDragDropHandler` (which only reads `CF_HDROP`
//! and hardcodes `DROPEFFECT_COPY`). This implementation:
//!
//! - Reads `CF_HDROP` (Explorer / real files)
//! - Reads `CFSTR_FILEDESCRIPTORW` + `CFSTR_FILECONTENTS` (WinRAR / virtual)
//! - Detects intra-Atlas drags via the `AtlasInternalSource` private format
//! - Sets `*pdwEffect` correctly (move vs copy via `DropZones` + key modifiers)
//! - Dispatches `Action` items through the shared queue back to the gpui `App`
//!
//! # Registration
//! `setup_dnd()` in the parent module calls `RevokeDragDrop` (removes gpui's
//! original target) and `RegisterDragDrop` with one `AtlasDropTarget` per window.
#![allow(unsafe_code)]
// reason: COM vtable, IDropTargetHelper and raw Win32 calls are inherently unsafe.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use windows::core::{implement, Result};
use windows::Win32::Foundation::{HWND, POINT, POINTL};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::System::Com::{IDataObject, DVASPECT_CONTENT, FORMATETC, TYMED_HGLOBAL};
use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
use windows::Win32::System::Ole::{
    IDropTarget, IDropTarget_Impl, RegisterDragDrop, ReleaseStgMedium, RevokeDragDrop, DROPEFFECT,
    DROPEFFECT_COPY, DROPEFFECT_NONE,
};
use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
use windows::Win32::UI::Shell::{CLSID_DragDropHelper, IDropTargetHelper};

use super::clipboard_formats::{cf_atlas_source, cf_file_contents, cf_file_descriptor_w};
use crate::dnd::DropZones;

// ─── Payload sent to the App ─────────────────────────────────────────────────

/// Resolved drop payload. Built inside the COM vtable, dispatched to App.
/// The move-vs-copy decision is made later in `drain_dnd_queue` (by volume),
/// including the modifier keys at the moment of release.
#[derive(Debug)]
pub struct DropPayload {
    pub paths: Vec<PathBuf>,
    /// True when the drag originated from Atlas itself (intra-Atlas move).
    pub from_self: bool,
    pub ctrl: bool,
    pub shift: bool,
    /// Window-space drop position (scaled, logical pixels).
    pub x: f32,
    pub y: f32,
}

// ─── Free helper functions (accessible from both impl blocks) ────────────────

/// Extract real filesystem paths from `CF_HDROP` data.
pub(super) unsafe fn read_cf_hdrop(data_obj: &IDataObject) -> Vec<PathBuf> {
    use windows::Win32::System::Ole::CF_HDROP;
    use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};
    let fmt = FORMATETC {
        cfFormat: CF_HDROP.0,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    };
    let Ok(mut medium) = data_obj.GetData(&fmt) else {
        return Vec::new();
    };
    if medium.u.hGlobal.is_invalid() {
        return Vec::new();
    }
    let hdrop = HDROP(medium.u.hGlobal.0);
    let count = DragQueryFileW(hdrop, 0xFFFF_FFFF, None);
    let mut paths = Vec::with_capacity(count as usize);
    for i in 0..count {
        let needed = DragQueryFileW(hdrop, i, None) as usize + 1;
        let mut buf = vec![0u16; needed];
        DragQueryFileW(hdrop, i, Some(&mut buf));
        buf.pop(); // remove null
        let s = String::from_utf16_lossy(&buf);
        paths.push(atlas_core::fs::paths::normalize_wsl_unc_path(
            PathBuf::from(s),
        ));
    }
    ReleaseStgMedium(&mut medium);
    paths
}

/// Check if the data object carries the Atlas-internal private format
/// (signalling an intra-Atlas drag).
pub(super) unsafe fn is_atlas_source(data_obj: &IDataObject) -> bool {
    let fmt = FORMATETC {
        cfFormat: cf_atlas_source(),
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    };
    data_obj.QueryGetData(&fmt).is_ok()
}

/// Extract virtual-file paths from `CFSTR_FILEDESCRIPTORW` + `CFSTR_FILECONTENTS`.
///
/// Each virtual file is extracted to `%TEMP%\atlas-dnd\<session_id>\<name>`.
/// Returns real paths to the extracted temporaries.
pub(super) unsafe fn read_virtual_files(data_obj: &IDataObject) -> Vec<PathBuf> {
    use windows::Win32::Foundation::HGLOBAL;
    use windows::Win32::System::Com::TYMED_ISTREAM;
    use windows::Win32::UI::Shell::FILEDESCRIPTORW;

    let desc_fmt = FORMATETC {
        cfFormat: cf_file_descriptor_w(),
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    };
    let Ok(mut desc_medium) = data_obj.GetData(&desc_fmt) else {
        return Vec::new();
    };
    if desc_medium.u.hGlobal.is_invalid() {
        return Vec::new();
    }

    // The HGLOBAL holds a FILEGROUPDESCRIPTORW: u32 cItems + cItems * FILEDESCRIPTORW.
    let ptr = GlobalLock(desc_medium.u.hGlobal) as *const u8;
    if ptr.is_null() {
        ReleaseStgMedium(&mut desc_medium);
        return Vec::new();
    }

    let count = (ptr as *const u32).read() as usize;
    let first_desc = ptr.add(std::mem::size_of::<u32>()) as *const FILEDESCRIPTORW;

    // Prepare the temp directory.
    let session_id = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_micros())
        .unwrap_or(0);
    let tmp_dir = std::env::temp_dir()
        .join("atlas-dnd")
        .join(session_id.to_string());
    let _ = std::fs::create_dir_all(&tmp_dir);

    let mut result = Vec::with_capacity(count);
    for i in 0..count {
        // FILEDESCRIPTORW is packed; copy cFileName to a local buffer before
        // taking any reference to avoid misaligned-reference UB.
        let desc_ptr = first_desc.add(i);
        let file_name_buf: [u16; 260] =
            std::ptr::read_unaligned(std::ptr::addr_of!((*desc_ptr).cFileName));
        let name = String::from_utf16_lossy(
            file_name_buf
                .iter()
                .copied()
                .take_while(|&c| c != 0)
                .collect::<Vec<u16>>()
                .as_slice(),
        );
        if name.is_empty() {
            continue;
        }

        // Request the stream for this item.
        let content_fmt = FORMATETC {
            cfFormat: cf_file_contents(),
            ptd: std::ptr::null_mut(),
            dwAspect: DVASPECT_CONTENT.0,
            lindex: i as i32,
            tymed: TYMED_HGLOBAL.0 as u32 | TYMED_ISTREAM.0 as u32,
        };
        let Ok(mut content_medium) = data_obj.GetData(&content_fmt) else {
            continue;
        };

        let dst = tmp_dir.join(&name);

        // Prefer ISTREAM (streaming, avoids loading everything into RAM).
        if content_medium.tymed == TYMED_ISTREAM.0 as u32 {
            if let Some(stream) = &*content_medium.u.pstm {
                if write_stream_to_file(stream, &dst).is_ok() {
                    result.push(dst);
                }
            }
        } else if content_medium.tymed == TYMED_HGLOBAL.0 as u32
            && !content_medium.u.hGlobal.is_invalid()
        {
            let hg = HGLOBAL(content_medium.u.hGlobal.0);
            let src = GlobalLock(hg) as *const u8;
            if !src.is_null() {
                let size = GlobalSize(hg);
                let data = std::slice::from_raw_parts(src, size);
                if std::fs::write(&dst, data).is_ok() {
                    result.push(dst);
                }
                let _ = GlobalUnlock(hg);
            }
        }

        ReleaseStgMedium(&mut content_medium);
    }

    let _ = GlobalUnlock(desc_medium.u.hGlobal);
    ReleaseStgMedium(&mut desc_medium);
    result
}

// ─── AtlasDropTarget ─────────────────────────────────────────────────────────

/// Atlas `IDropTarget` registered over the gpui window HWND.
///
/// `zones` and `action_queue` are `Arc`-shared with the `App` entity:
/// - `zones` is updated every frame by the render path (when a drag is active).
/// - Completed drops are pushed to `action_queue`; `App` drains it on the next
///   `notify` cycle.
#[implement(IDropTarget)]
pub struct AtlasDropTarget {
    hwnd: HWND,
    scale_factor: f32,
    zones: Arc<Mutex<DropZones>>,
    /// Event queue: the COM vtable pushes; `App::drain_dnd_queue` pops.
    action_queue: super::DndQueue,
    /// Shell helper for translating drag images.
    helper: Option<IDropTargetHelper>,
    /// Last computed effect (used in DragOver to remember decision).
    last_effect: std::cell::Cell<DROPEFFECT>,
    active_data: std::cell::RefCell<Option<IDataObject>>,
    description_target: std::cell::RefCell<Option<(crate::dnd::DropEffect, PathBuf)>>,
}

impl AtlasDropTarget {
    /// Construct and immediately call `RegisterDragDrop` for `hwnd`.
    ///
    /// `scale_factor` converts Win32 screen points to logical gpui pixels.
    /// Returns the `IDropTarget` COM object (keeps itself alive via refcount).
    pub fn register(
        hwnd: HWND,
        scale_factor: f32,
        zones: Arc<Mutex<DropZones>>,
        action_queue: super::DndQueue,
    ) -> Result<IDropTarget> {
        // Remove gpui's target first.
        unsafe {
            let _ = RevokeDragDrop(hwnd); // ignore error (may not be registered)
        }

        let helper: Option<IDropTargetHelper> = unsafe {
            windows::Win32::System::Com::CoCreateInstance(
                &CLSID_DragDropHelper,
                None,
                windows::Win32::System::Com::CLSCTX_INPROC_SERVER,
            )
            .ok()
        };

        let target: IDropTarget = AtlasDropTarget {
            hwnd,
            scale_factor,
            zones,
            action_queue,
            helper,
            last_effect: std::cell::Cell::new(DROPEFFECT_NONE),
            active_data: Default::default(),
            description_target: Default::default(),
        }
        .into();

        unsafe { RegisterDragDrop(hwnd, &target) }?;
        Ok(target)
    }

    /// Current scale-corrected logical position from a Win32 screen `POINTL`.
    fn to_logical(&self, pt: &POINTL) -> (f32, f32) {
        let mut p = POINT { x: pt.x, y: pt.y };
        unsafe {
            let _ = ScreenToClient(self.hwnd, &mut p);
        }
        let dpi = unsafe { windows::Win32::UI::HiDpi::GetDpiForWindow(self.hwnd) };
        let scale = if dpi == 0 { self.scale_factor } else { dpi as f32 / 96.0 };
        (p.x as f32 / scale, p.y as f32 / scale)
    }

    fn update_description(&self, x: f32, y: f32, ctrl: bool, shift: bool) -> DROPEFFECT {
        let target = self.zones.lock().or_else(atlas_model::sync::recover).ok().and_then(|zones| {
            let dest = zones.hover_snapshot.as_ref()?.transfer_at(x, y, &zones.paths, ctrl, shift)?;
            let default = if zones.paths.is_empty() { crate::dnd::DropEffect::Copy }
                else { crate::dnd::default_effect_for_paths(&zones.paths, &dest) };
            Some((crate::dnd::effect_with_modifiers(default, ctrl, shift), dest))
        });
        if *self.description_target.borrow() != target {
            let object = self.active_data.borrow().clone();
            if let Some(object) = object.as_ref() {
                super::drop_description::publish(object, target.as_ref().map(|t| t.0), target.as_ref().map(|t| t.1.as_path()));
            }
            *self.description_target.borrow_mut() = target.clone();
        }
        match target.map(|(effect, _)| effect) {
            Some(crate::dnd::DropEffect::Move) => windows::Win32::System::Ole::DROPEFFECT_MOVE,
            Some(crate::dnd::DropEffect::Copy) => DROPEFFECT_COPY,
            None => DROPEFFECT_NONE,
        }
    }

    fn queue_motion(&self, x: f32, y: f32) {
        if let Some(mut q) = self.action_queue.lock().or_else(atlas_model::sync::recover).ok() {
            let motion = super::DndEvent::DragOver { x, y };
            if let Some(last @ super::DndEvent::DragOver { .. }) = q.last_mut() {
                *last = motion;
            } else {
                q.push(motion);
            }
            crate::ui_wake::wake();
        }
    }

    fn queue_leave(&self) {
        if let Some(mut q) = self.action_queue.lock().or_else(atlas_model::sync::recover).ok() {
            q.push(super::DndEvent::DragLeave);
            crate::ui_wake::wake();
        }
    }

    fn clear_description(&self) {
        let object = self.active_data.borrow_mut().take();
        if let Some(object) = object {
            super::drop_description::publish(&object, None, None);
        }
        self.description_target.borrow_mut().take();
    }
}

impl IDropTarget_Impl for AtlasDropTarget_Impl {
    fn DragEnter(
        &self,
        pdataobj: windows::core::Ref<IDataObject>,
        grfkeystate: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        pdweffect: *mut DROPEFFECT,
    ) -> Result<()> {
        let Some(data_obj) = pdataobj.as_ref() else {
            unsafe { *pdweffect = DROPEFFECT_NONE };
            return Ok(());
        };
        self.clear_description();
        *self.active_data.borrow_mut() = Some(data_obj.clone());
        super::drop_description::publish(data_obj, None, None);

        // Accept if the source offers real files (CF_HDROP) or virtual files
        // (CFSTR_FILEDESCRIPTORW — WinRAR, 7-Zip, Outlook attachments, etc.).
        use windows::Win32::System::Ole::CF_HDROP;
        let hdrop_fmt = FORMATETC {
            cfFormat: CF_HDROP.0,
            ptd: std::ptr::null_mut(),
            dwAspect: DVASPECT_CONTENT.0,
            lindex: -1,
            tymed: TYMED_HGLOBAL.0 as u32,
        };
        let virt_fmt = FORMATETC {
            cfFormat: cf_file_descriptor_w(),
            ptd: std::ptr::null_mut(),
            dwAspect: DVASPECT_CONTENT.0,
            lindex: -1,
            tymed: TYMED_HGLOBAL.0 as u32,
        };
        let effect = unsafe {
            if data_obj.QueryGetData(&hdrop_fmt).is_ok() || data_obj.QueryGetData(&virt_fmt).is_ok()
            {
                DROPEFFECT_COPY
            } else {
                DROPEFFECT_NONE
            }
        };
        self.last_effect.set(effect);
        unsafe { *pdweffect = effect };
        if effect == DROPEFFECT_NONE { return Ok(()); }

        let (x, y) = self.to_logical(pt);
        let paths = unsafe { read_cf_hdrop(data_obj) };
        if let Some(mut zones) = self.zones.lock().or_else(atlas_model::sync::recover).ok() {
            zones.drag_active = true;
            zones.cursor = Some((x, y));
            zones.paths = paths;
            zones.ctrl = grfkeystate.0 & 0x0008 != 0;
            zones.shift = grfkeystate.0 & 0x0004 != 0;
        }
        self.queue_motion(x, y);

        let visual_effect = self.update_description(x, y, grfkeystate.0 & 0x0008 != 0, grfkeystate.0 & 0x0004 != 0);
        if let Some(helper) = &self.helper {
            unsafe {
                match helper.DragEnter(self.hwnd, data_obj, &POINT { x: pt.x, y: pt.y }, visual_effect) {
                    Ok(()) => { if let Err(error) = helper.Show(true) {
                        tracing::debug!(?error, "Shell drag image Show failed");
                    } },
                    Err(error) => tracing::debug!(?error, "Shell drag image DragEnter failed"),
                }
            }
        }
        Ok(())
    }

    fn DragOver(
        &self,
        grfkeystate: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        pdweffect: *mut DROPEFFECT,
    ) -> Result<()> {
        // Echo back what DragEnter decided — same as Zed's approach.
        // Move vs copy is resolved in drain_dnd_queue using volume comparison.
        let effect = self.last_effect.get();
        unsafe { *pdweffect = effect };
        if effect == DROPEFFECT_NONE { return Ok(()); }

        let (x, y) = self.to_logical(pt);
        let visual_effect = self.update_description(x, y, grfkeystate.0 & 0x0008 != 0, grfkeystate.0 & 0x0004 != 0);
        if let Some(helper) = &self.helper {
            unsafe {
                let _ = helper.DragOver(&POINT { x: pt.x, y: pt.y }, visual_effect);
            }
        }
        if let Some(mut zones) = self.zones.lock().or_else(atlas_model::sync::recover).ok() {
            zones.cursor = Some((x, y));
            zones.ctrl = grfkeystate.0 & 0x0008 != 0;
            zones.shift = grfkeystate.0 & 0x0004 != 0;
        }
        self.queue_motion(x, y);
        Ok(())
    }

    fn DragLeave(&self) -> Result<()> {
        self.clear_description();
        if let Some(mut zones) = self.zones.lock().or_else(atlas_model::sync::recover).ok() {
            zones.drag_active = false;
            zones.cursor = None;
            zones.paths.clear();
            zones.hover_snapshot = None;
        }
        self.queue_leave();
        if let Some(helper) = &self.helper {
            unsafe {
                let _ = helper.DragLeave();
            }
        }
        Ok(())
    }

    fn Drop(
        &self,
        pdataobj: windows::core::Ref<IDataObject>,
        grfkeystate: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        pdweffect: *mut DROPEFFECT,
    ) -> Result<()> {
        self.clear_description();
        let snapshot = self.zones.lock().or_else(atlas_model::sync::recover).ok().and_then(|mut zones| {
            zones.drag_active = false;
            zones.cursor = None;
            zones.paths.clear();
            zones.hover_snapshot.take()
        });

        let Some(data_obj) = pdataobj.as_ref() else {
            self.queue_leave();
            unsafe { *pdweffect = DROPEFFECT_NONE };
            return Ok(());
        };
        let (lx, ly) = self.to_logical(pt);

        let ctrl = grfkeystate.0 & 0x0008 != 0;
        let shift = grfkeystate.0 & 0x0004 != 0;
        let from_self = unsafe { is_atlas_source(data_obj) };

        // Try virtual format first (WinRAR, 7-Zip, Outlook attachments…).
        let paths = unsafe {
            let virt = read_virtual_files(data_obj);
            if !virt.is_empty() {
                virt
            } else {
                read_cf_hdrop(data_obj)
            }
        };

        let blocked = paths.is_empty() || snapshot
            .and_then(|snapshot| snapshot.command_at(lx, ly))
            .is_some_and(|command| crate::ui::drag_hover::drop_is_structurally_blocked(
                &command, &paths, ctrl, shift,
            ));
        // Report COPY for a submitted drop so the source never removes the
        // files itself. The transfer dispatcher owns the copy or move.
        let effect = if blocked { DROPEFFECT_NONE } else { DROPEFFECT_COPY };
        if let Some(helper) = &self.helper {
            unsafe {
                let _ = helper.Drop(data_obj, &POINT { x: pt.x, y: pt.y }, effect);
            }
        }
        unsafe { *pdweffect = effect };
        if blocked {
            self.queue_leave();
            return Ok(());
        }
        let payload = DropPayload {
            paths,
            from_self,
            ctrl,
            shift,
            x: lx,
            y: ly,
        };
        if let Some(mut q) = self.action_queue.lock().or_else(atlas_model::sync::recover).ok() {
            q.push(super::DndEvent::DropIn(payload));
            crate::ui_wake::wake();
        }
        Ok(())
    }
}

// ─── Utility ─────────────────────────────────────────────────────────────────

/// Copy an `IStream` to a file path on disk.
unsafe fn write_stream_to_file(
    stream: &windows::Win32::System::Com::IStream,
    dst: &std::path::Path,
) -> std::io::Result<()> {
    use windows::Win32::System::Com::STREAM_SEEK_SET;
    // Rewind.
    let _ = stream.Seek(0, STREAM_SEEK_SET, None);
    let mut file = std::fs::File::create(dst)?;
    let mut buf = vec![0u8; 64 * 1024]; // 64 KiB chunks
    loop {
        let mut read: u32 = 0;
        let hr = stream.Read(
            buf.as_mut_ptr() as *mut _,
            buf.len() as u32,
            Some(&mut read),
        );
        if read == 0 {
            break;
        }
        use std::io::Write;
        file.write_all(&buf[..read as usize])?;
        if hr.is_err() {
            break;
        }
    }
    Ok(())
}
