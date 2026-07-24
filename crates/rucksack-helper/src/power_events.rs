use anyhow::{anyhow, Result};
#[cfg(target_os = "macos")]
use std::sync::mpsc;
use std::sync::mpsc::Sender;
use std::thread::JoinHandle;
#[cfg(target_os = "macos")]
use std::time::Duration;

#[cfg(target_os = "macos")]
const OBSERVER_START_TIMEOUT: Duration = Duration::from_secs(5);

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use std::ffi::c_void;

    type CFRunLoopRef = *const c_void;
    type CFRunLoopSourceRef = *const c_void;
    type CFStringRef = *const c_void;

    #[link(name = "IOKit", kind = "framework")]
    extern "C" {
        fn IOPSNotificationCreateRunLoopSource(
            callback: extern "C" fn(*mut c_void),
            context: *mut c_void,
        ) -> CFRunLoopSourceRef;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        static kCFRunLoopDefaultMode: CFStringRef;
        fn CFRunLoopGetCurrent() -> CFRunLoopRef;
        fn CFRunLoopAddSource(
            run_loop: CFRunLoopRef,
            source: CFRunLoopSourceRef,
            mode: CFStringRef,
        );
        fn CFRunLoopRemoveSource(
            run_loop: CFRunLoopRef,
            source: CFRunLoopSourceRef,
            mode: CFStringRef,
        );
        fn CFRunLoopRun();
        fn CFRelease(value: *const c_void);
    }

    extern "C" fn callback(context: *mut c_void) {
        if context.is_null() {
            return;
        }
        let sender = unsafe { &*(context as *const Sender<()>) };
        let _ = sender.send(());
    }

    pub fn spawn(sender: Sender<()>) -> Result<JoinHandle<()>> {
        let (ready_sender, ready_receiver) =
            mpsc::sync_channel::<std::result::Result<(), String>>(1);
        let handle = std::thread::Builder::new()
            .name("rucksack-power-source-listener".to_owned())
            .spawn(move || unsafe {
                let context = Box::into_raw(Box::new(sender)) as *mut c_void;
                let source = IOPSNotificationCreateRunLoopSource(callback, context);
                if source.is_null() {
                    drop(Box::from_raw(context as *mut Sender<()>));
                    let _ = ready_sender.send(Err(
                        "IOPSNotificationCreateRunLoopSource returned null".to_owned(),
                    ));
                    return;
                }
                let run_loop = CFRunLoopGetCurrent();
                if run_loop.is_null() {
                    CFRelease(source);
                    drop(Box::from_raw(context as *mut Sender<()>));
                    let _ = ready_sender.send(Err("CFRunLoopGetCurrent returned null".to_owned()));
                    return;
                }
                CFRunLoopAddSource(run_loop, source, kCFRunLoopDefaultMode);
                if ready_sender.send(Ok(())).is_err() {
                    CFRunLoopRemoveSource(run_loop, source, kCFRunLoopDefaultMode);
                    CFRelease(source);
                    drop(Box::from_raw(context as *mut Sender<()>));
                    return;
                }
                CFRunLoopRun();
                CFRunLoopRemoveSource(run_loop, source, kCFRunLoopDefaultMode);
                CFRelease(source);
                drop(Box::from_raw(context as *mut Sender<()>));
            })
            .map_err(|error| anyhow!("Could not spawn power observer: {error}"))?;

        match ready_receiver.recv_timeout(OBSERVER_START_TIMEOUT) {
            Ok(Ok(())) => Ok(handle),
            Ok(Err(message)) => {
                handle
                    .join()
                    .map_err(|_| anyhow!("Power observer panicked during startup"))?;
                Err(anyhow!("Power observer startup failed: {message}"))
            }
            Err(error) => Err(anyhow!(
                "Power observer did not report readiness within {} ms: {error}",
                OBSERVER_START_TIMEOUT.as_millis()
            )),
        }
    }
}

#[cfg(target_os = "macos")]
pub use macos::spawn;

#[cfg(not(target_os = "macos"))]
pub fn spawn(_sender: Sender<()>) -> Result<JoinHandle<()>> {
    Err(anyhow!(
        "Power-source observation is available only on macOS"
    ))
}
