//! The Windows Share sheet (DataTransferManager) for a file. The file is handed over
//! as a storage item; Windows and the chosen app take it from there. Nothing is sent
//! anywhere unless the user picks a target.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;

use windows::ApplicationModel::DataTransfer::{DataRequestedEventArgs, DataTransferManager};
use windows::Foundation::TypedEventHandler;
use windows::Storage::{IStorageItem, StorageFile};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Shell::IDataTransferManagerInterop;
use windows::core::{HSTRING, Interface, Ref};
use windows_collections::IIterable;

thread_local! {
    /// One DataRequested handler per window; the previous one is removed first.
    static HANDLERS: RefCell<HashMap<isize, (DataTransferManager, i64)>> = RefCell::new(HashMap::new());
}

fn err(e: windows::core::Error) -> String {
    let m = e.message();
    if m.is_empty() { format!("0x{:08X}", e.code().0 as u32) } else { m }
}

/// Opens the Share sheet for `path` over the window `hwnd`.
pub fn share_file(hwnd: isize, path: &Path, title: &str) -> Result<(), String> {
    let interop = windows::core::factory::<DataTransferManager, IDataTransferManagerInterop>().map_err(err)?;
    let window = HWND(hwnd as *mut _);
    let dtm: DataTransferManager = unsafe { interop.GetForWindow(window) }.map_err(err)?;
    HANDLERS.with(|h| {
        if let Some((old, token)) = h.borrow_mut().remove(&hwnd) {
            let _ = old.RemoveDataRequested(token);
        }
    });
    let file = HSTRING::from(path.as_os_str());
    let title = HSTRING::from(title);
    let handler = TypedEventHandler::new(move |_: Ref<DataTransferManager>, args: Ref<DataRequestedEventArgs>| {
        let Some(args) = args.as_ref() else { return Ok(()) };
        let request = args.Request()?;
        let data = request.Data()?;
        data.Properties()?.SetTitle(&title)?;
        // Opening a StorageFile is asynchronous: hold a deferral while it resolves.
        let deferral = request.GetDeferral()?;
        let op = StorageFile::GetFileFromPathAsync(&file)?;
        std::thread::spawn(move || {
            let result = (|| -> windows::core::Result<()> {
                let f = op.join()?;
                let items: IIterable<IStorageItem> = vec![Some(f.cast::<IStorageItem>()?)].into();
                data.SetStorageItemsReadOnly(&items)
            })();
            if let Err(e) = result {
                let _ = request.FailWithDisplayText(&HSTRING::from(format!("The file couldn’t be shared: {}", err(e))));
            }
            let _ = deferral.Complete();
        });
        Ok(())
    });
    let token = dtm.DataRequested(&handler).map_err(err)?;
    HANDLERS.with(|h| h.borrow_mut().insert(hwnd, (dtm, token)));
    unsafe { interop.ShowShareUIForWindow(window) }.map_err(err)
}
