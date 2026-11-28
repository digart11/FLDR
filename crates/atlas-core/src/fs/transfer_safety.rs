#![allow(unsafe_code)]
//! Narrow Windows ownership primitives. No path-based removal or replacing rename.
use std::{fs::{File,Metadata,OpenOptions},io::{self,Read,Write,Seek,SeekFrom},path::{Path,PathBuf},sync::atomic::{AtomicBool,Ordering}};
use atlas_model::undo::{ObjectStamp,TransferUndo};
use sha2::{Digest,Sha256};

pub fn ordinary(meta:&Metadata)->io::Result<()> {
    #[cfg(windows)] {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 {return Err(io::Error::new(io::ErrorKind::Unsupported,"reparse points (including junctions and symbolic links) are not supported"));}
    }
    if meta.file_type().is_symlink() || (!meta.is_file() && !meta.is_dir()) {
        return Err(io::Error::new(io::ErrorKind::Unsupported,"unsupported filesystem entry"));
    }
    Ok(())
}
pub fn inspect(path:&Path)->io::Result<()> {ordinary(&std::fs::symlink_metadata(path)?)}

/// No-follow enumeration: any unreadable or unsupported entry invalidates completeness.
pub fn tree(path:&Path)->io::Result<Vec<(PathBuf,u64,std::time::SystemTime,(u64,u64))>> {
    fn visit(path:&Path,depth:usize,result:&mut Vec<(PathBuf,u64,std::time::SystemTime,(u64,u64))>)->io::Result<()> {
        if depth>128 {return Err(io::Error::new(io::ErrorKind::Unsupported,"tree exceeds safe inspection depth"));}
        inspect(path)?;
        let is_dir=std::fs::symlink_metadata(path)?.is_dir();
        let file=open(path,false,is_dir)?;let meta=file.metadata()?;ordinary(&meta)?;
        result.push((path.to_path_buf(),meta.len(),meta.modified()?,identity(&file)?));
        if meta.is_dir() {for child in std::fs::read_dir(path)? {visit(&child?.path(),depth+1,result)?;}}
        Ok(())
    }
    let _parents=parents(path)?;let mut result=Vec::new();visit(path,0,&mut result)?;
    result.sort_by(|a,b|a.0.cmp(&b.0));Ok(result)
}
#[cfg(windows)]
fn open(path:&Path, delete:bool, directory:bool)->io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    let access=if directory {0x80} else {0x80000000} | if delete {0x10000} else {0};
    let file=OpenOptions::new().access_mode(access).share_mode(1)
        .custom_flags(0x00200000 | if directory {0x02000000} else {0}).open(path)?;
    ordinary(&file.metadata()?)?;Ok(file)
}
#[cfg(not(windows))]
fn open(_: &Path,_:bool,_:bool)->io::Result<File> {Err(io::Error::new(io::ErrorKind::Unsupported,"guarded transfers require Windows ownership primitives on this platform"))}

/// Keep every existing ancestor open without write/delete sharing. Reject reparse
/// ancestors rather than resolving through them. The leases live across mutation.
pub struct Parents { _files:Vec<File> }
pub fn parents(path:&Path)->io::Result<Parents> {
    if !path.is_absolute() || path.components().any(|part|matches!(part,std::path::Component::ParentDir)) {
        return Err(io::Error::new(io::ErrorKind::Unsupported,"an absolute path without parent traversal is required"));
    }
    let mut chain:Vec<_>=path.parent().ok_or_else(||io::Error::other("no parent"))?.ancestors().collect();chain.reverse();
    let mut files=Vec::new();for parent in chain {files.push(open(parent,false,true)?);}
    Ok(Parents{_files:files})
}
#[cfg(windows)]
fn identity(file:&File)->io::Result<(u64,u64)> {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::{Foundation::HANDLE,Storage::FileSystem::{GetFileInformationByHandle,BY_HANDLE_FILE_INFORMATION}};
    let mut info=BY_HANDLE_FILE_INFORMATION::default();
    unsafe {GetFileInformationByHandle(HANDLE(file.as_raw_handle()),&mut info)}.map_err(io::Error::other)?;
    Ok((info.dwVolumeSerialNumber as u64,((info.nFileIndexHigh as u64)<<32)|info.nFileIndexLow as u64))
}
#[cfg(not(windows))]
fn identity(_: &File)->io::Result<(u64,u64)> {Err(io::Error::new(io::ErrorKind::Unsupported,"file identity unavailable"))}
fn stamp(file:&mut File)->io::Result<ObjectStamp> {
    let meta=file.metadata()?;ordinary(&meta)?;
    if !meta.is_file() {return Err(io::Error::new(io::ErrorKind::Unsupported,"directory inverse requires a created-item manifest"));}
    file.seek(SeekFrom::Start(0))?;let mut hash=Sha256::new();let mut buf=[0u8;65536];
    loop {let n=file.read(&mut buf)?;if n==0 {break;}hash.update(&buf[..n]);}
    Ok(ObjectStamp {identity:identity(file)?,size:meta.len(),modified:meta.modified()?,digest:hash.finalize().into()})
}
/// Atomically create an owned output. A failure leaves an explicitly incomplete
/// file; never unlink by pathname, even if this function originally created it.
pub fn copy_file(src:&Path,dst:&Path,cancel:&AtomicBool,progress:impl Fn(u64))->io::Result<CreatedFile> {
    let _source_parents=parents(src)?;let _destination_parents=parents(dst)?;
    let mut source=open(src,false,false)?;
    if cancel.load(Ordering::Relaxed) {return Err(io::Error::new(io::ErrorKind::Interrupted,"cancelled"));}
    let mut options=OpenOptions::new();options.read(true).write(true).create_new(true);
    #[cfg(windows)] {use std::os::windows::fs::OpenOptionsExt;options.share_mode(1).custom_flags(0x00200000);}
    let mut target=options.open(dst)?;ordinary(&target.metadata()?)?;
    let owned_identity=identity(&target)?;
    let mut hash=Sha256::new();
    let mut total=0;let mut buf=vec![0u8;1024*1024];
    loop {
        if cancel.load(Ordering::Relaxed) {return Err(io::Error::new(io::ErrorKind::Interrupted,"cancelled; owned partial output retained"));}
        let n=source.read(&mut buf)?;if n==0 {break;}
        target.write_all(&buf[..n])?;hash.update(&buf[..n]);total+=n as u64;progress(total);
    }
    target.sync_all()?;
    // Source is write/delete locked throughout the read. No metadata-fidelity claim.
    if source.metadata()?.len()!=total {return Err(io::Error::other("source length changed"));}
    Ok(CreatedFile {bytes:total,identity:owned_identity,digest:hash.finalize().into()})
}
/// Bind an Undo certificate to the bytes in an independently locked source and
/// destination. Missing/changed/ambiguous identity simply suppresses Undo.
#[derive(Clone)]
pub struct CreatedFile {pub bytes:u64,identity:(u64,u64),digest:[u8;32]}
pub fn copy_certificate(src:&Path,dst:&Path,created:&CreatedFile)->io::Result<TransferUndo> {
    let _parents=parents(dst)?;let mut destination=open(dst,false,false)?;
    let copied=stamp(&mut destination)?;
    if copied.identity!=created.identity || copied.size!=created.bytes || copied.digest!=created.digest {
        return Err(io::Error::other("destination ownership changed before certification"));
    }
    Ok(TransferUndo {source:src.to_path_buf(),destination:dst.to_path_buf(),stamp:copied})
}
#[cfg(windows)]
fn rename_handle(file:&File,dst:&Path)->io::Result<()> {
    use std::os::windows::{ffi::OsStrExt,io::AsRawHandle};
    use windows::Win32::{Foundation::HANDLE,Storage::FileSystem::{SetFileInformationByHandle,FileRenameInfo,FILE_RENAME_INFO}};
    let name:Vec<u16>=dst.as_os_str().encode_wide().collect();
    let bytes=std::mem::offset_of!(FILE_RENAME_INFO,FileName)+name.len()*2;
    let mut storage=vec![0u64;bytes.div_ceil(8)];
    unsafe {
        let info=storage.as_mut_ptr().cast::<FILE_RENAME_INFO>();
        (*info).Anonymous.ReplaceIfExists=false;(*info).FileNameLength=(name.len()*2) as u32;
        std::ptr::copy_nonoverlapping(name.as_ptr(),std::ptr::addr_of_mut!((*info).FileName).cast::<u16>(),name.len());
        SetFileInformationByHandle(HANDLE(file.as_raw_handle()),FileRenameInfo,info.cast(),bytes as u32)
    }.map_err(io::Error::other)
}
#[cfg(not(windows))]
fn rename_handle(_: &File,_:&Path)->io::Result<()> {Err(io::Error::new(io::ErrorKind::Unsupported,"no-clobber handle rename unavailable"))}
pub fn rename_new(src:&Path,dst:&Path)->io::Result<Option<TransferUndo>> {
    let _a=parents(src)?;let _b=parents(dst)?;inspect(src)?;
    let directory=std::fs::symlink_metadata(src)?.is_dir();
    let mut file=open(src,true,directory)?;
    rename_handle(&file,dst)?;
    if directory {Ok(None)} else {
        Ok(stamp(&mut file).ok().map(|stamp|TransferUndo{source:src.to_path_buf(),destination:dst.to_path_buf(),stamp}))
    }
}
/// This is the only inverse for certified transfers. It never recursively
/// removes a directory or performs path-based delete/replace.
pub fn undo(item:&TransferUndo,kind:atlas_model::ops::TransferKind)->io::Result<()> {
    let _a=parents(&item.destination)?;let _b=parents(&item.source)?;
    let mut file=open(&item.destination,true,false)?;
    if stamp(&mut file)?!=item.stamp {return Err(io::Error::other("Undo ownership/version could not be established"));}
    if kind==atlas_model::ops::TransferKind::Move {rename_handle(&file,&item.source)} else {
        #[cfg(windows)] {
            use std::os::windows::io::AsRawHandle;
            use windows::Win32::{Foundation::HANDLE,Storage::FileSystem::{SetFileInformationByHandle,FileDispositionInfo,FILE_DISPOSITION_INFO}};
            let disposition=FILE_DISPOSITION_INFO {DeleteFile:true};
            unsafe {SetFileInformationByHandle(HANDLE(file.as_raw_handle()),FileDispositionInfo,(&disposition as *const FILE_DISPOSITION_INFO).cast(),std::mem::size_of_val(&disposition) as u32)}.map_err(io::Error::other)
        }
        #[cfg(not(windows))] {Err(io::Error::new(io::ErrorKind::Unsupported,"owned delete unavailable"))}
    }
}

/// This classification never authorizes deletion: copied moves retain sources
/// even when the observed identity/version is unchanged.
pub fn retained_reason(path:&Path,before:&[(PathBuf,u64,std::time::SystemTime,(u64,u64))])->atlas_model::ops::OpErrorKind {
    if tree(path).map_or(true,|after|after!=before) {atlas_model::ops::OpErrorKind::SourceChanged}
    else {atlas_model::ops::OpErrorKind::SourceNotRemoved("copy/delete removal is disabled until stable-source and metadata equivalence can be proven".into())}
}
