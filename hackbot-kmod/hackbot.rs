// SPDX-License-Identifier: GPL-2.0

//! hackbot — In-kernel LLM agent character device.
//!
//! Step 1: Module skeleton with /dev/hackbot prompt/response interface.
//! No inference engine yet — the "brain" is a dummy that echoes back.
//!
//! Usage:
//! ```sh
//! sudo insmod hackbot.ko
//! echo "what processes are running?" > /dev/hackbot
//! cat /dev/hackbot
//! # Output: [hackbot] I received: what processes are running?
//! sudo rmmod hackbot
//! ```

use core::pin::Pin;

use kernel::{
    c_str,
    device::Device,
    fs::{File, Kiocb},
    iov::{IovIterDest, IovIterSource},
    miscdevice::{MiscDevice, MiscDeviceOptions, MiscDeviceRegistration},
    new_condvar, new_mutex,
    prelude::*,
    sync::{aref::ARef, CondVar, Mutex},
};

module! {
    type: HackbotModule,
    name: "hackbot",
    authors: ["Sunwoo Jang"],
    description: "In-kernel LLM agent — Step 1: prompt/response skeleton",
    license: "GPL",
}

// ---------------------------------------------------------------------------
// Module registration
// ---------------------------------------------------------------------------

#[pin_data]
struct HackbotModule {
    #[pin]
    _miscdev: MiscDeviceRegistration<HackbotDev>,
}

impl kernel::InPlaceModule for HackbotModule {
    fn init(_module: &'static ThisModule) -> impl PinInit<Self, Error> {
        pr_info!("hackbot: loading module, creating /dev/hackbot\n");

        let options = MiscDeviceOptions {
            name: c_str!("hackbot"),
        };

        try_pin_init!(Self {
            _miscdev <- MiscDeviceRegistration::register(options),
        })
    }
}

impl Drop for HackbotModule {
    fn drop(&mut self) {
        pr_info!("hackbot: unloading module\n");
    }
}

// ---------------------------------------------------------------------------
// Per-fd state: each open("/dev/hackbot") gets its own prompt/response buffer
// ---------------------------------------------------------------------------

/// Shared mutable state protected by a mutex.
struct Inner {
    /// The prompt written by userspace.
    prompt: KVVec<u8>,
    /// The response generated for the prompt.
    response: KVVec<u8>,
    /// Whether a response is ready to be read.
    response_ready: bool,
}

/// Per-file-descriptor device state.
#[pin_data(PinnedDrop)]
struct HackbotDev {
    #[pin]
    inner: Mutex<Inner>,
    #[pin]
    response_cv: CondVar,
    dev: ARef<Device>,
}

// ---------------------------------------------------------------------------
// The "brain" — dummy for Step 1, will become LLM inference in Step 2+
// ---------------------------------------------------------------------------

/// Process a prompt and generate a response.
/// Step 1: simply echoes back the prompt with a prefix.
/// Step 2+: this will be replaced with actual LLM inference.
fn process_prompt(prompt: &[u8], response: &mut KVVec<u8>) {
    response.clear();

    let prefix = b"[hackbot] I received: ";
    // Best-effort: ignore allocation failures for the response buffer
    let _ = response.extend_from_slice(prefix, GFP_KERNEL);

    // Echo the prompt, stripping any trailing newline
    let prompt_trimmed = if prompt.last() == Some(&b'\n') {
        &prompt[..prompt.len() - 1]
    } else {
        prompt
    };
    let _ = response.extend_from_slice(prompt_trimmed, GFP_KERNEL);
    let _ = response.extend_from_slice(b"\n", GFP_KERNEL);
}

// ---------------------------------------------------------------------------
// MiscDevice trait implementation — the file_operations vtable
// ---------------------------------------------------------------------------

#[vtable]
impl MiscDevice for HackbotDev {
    type Ptr = Pin<KBox<Self>>;

    fn open(_file: &File, misc: &MiscDeviceRegistration<Self>) -> Result<Pin<KBox<Self>>> {
        let dev = ARef::from(misc.device());
        dev_info!(dev, "hackbot: device opened\n");

        KBox::try_pin_init(
            try_pin_init! {
                HackbotDev {
                    inner <- new_mutex!(Inner {
                        prompt: KVVec::new(),
                        response: KVVec::new(),
                        response_ready: false,
                    }),
                    response_cv <- new_condvar!(),
                    dev: dev,
                }
            },
            GFP_KERNEL,
        )
    }

    /// Handle write() from userspace — receives a prompt.
    fn write_iter(mut kiocb: Kiocb<'_, Self::Ptr>, iov: &mut IovIterSource<'_>) -> Result<usize> {
        let me = kiocb.file();

        let mut inner = me.inner.lock();

        // Read the prompt from userspace.
        inner.prompt.clear();
        let len = iov.copy_from_iter_vec(&mut inner.prompt, GFP_KERNEL)?;

        if len == 0 {
            return Ok(0);
        }

        // Process the prompt (dummy echo for Step 1).
        // We need a temporary buffer because process_prompt reads from prompt
        // and writes to response, and both are in the same Inner struct.
        let mut response = KVVec::new();
        process_prompt(&inner.prompt, &mut response);
        inner.response = response;
        inner.response_ready = true;

        // Signal any readers waiting for a response.
        me.response_cv.notify_all();

        // Reset file position so next read starts from the beginning.
        *kiocb.ki_pos_mut() = 0;

        Ok(len)
    }

    /// Handle read() from userspace — returns the response.
    /// Blocks until a response is available (interruptible).
    fn read_iter(mut kiocb: Kiocb<'_, Self::Ptr>, iov: &mut IovIterDest<'_>) -> Result<usize> {
        let me = kiocb.file();

        let mut inner = me.inner.lock();

        // Wait for a response to be ready.
        while !inner.response_ready {
            if me.response_cv.wait_interruptible(&mut inner) {
                // Signal pending — return ERESTARTSYS so the syscall can be restarted.
                return Err(ERESTARTSYS);
            }
        }

        // Copy response to userspace, respecting file position.
        let read = iov.simple_read_from_buffer(kiocb.ki_pos_mut(), &inner.response)?;

        // If we've read everything (position >= response length), mark as consumed
        // so the next read will block again until a new prompt arrives.
        if *kiocb.ki_pos_mut() >= inner.response.len() as i64 {
            inner.response_ready = false;
        }

        Ok(read)
    }
}

#[pinned_drop]
impl PinnedDrop for HackbotDev {
    fn drop(self: Pin<&mut Self>) {
        dev_info!(self.dev, "hackbot: device closed\n");
    }
}
