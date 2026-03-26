// SPDX-License-Identifier: GPL-2.0

//! MiscDevice trait impl: HackbotModule, HackbotDev, read/write handlers.

use core::pin::Pin;

use kernel::{
    c_str, dev_info,
    device::Device,
    fs::{File, Kiocb},
    iov::{IovIterDest, IovIterSource},
    miscdevice::{MiscDevice, MiscDeviceOptions, MiscDeviceRegistration},
    prelude::*,
    sync::aref::ARef,
};

use crate::config::*;
use crate::model::{free_model_resources, load_model_if_needed};
use crate::state::{MODEL, RESPONSE};
use crate::vllm::process_prompt;

// ---------------------------------------------------------------------------
// Module registration
// ---------------------------------------------------------------------------

#[pin_data(PinnedDrop)]
pub(crate) struct HackbotModule {
    #[pin]
    _miscdev: MiscDeviceRegistration<HackbotDev>,
}

impl kernel::InPlaceModule for HackbotModule {
    fn init(_module: &'static ThisModule) -> impl PinInit<Self, Error> {
        // SAFETY: Called exactly once during module load.
        unsafe { RESPONSE.init() };
        unsafe { MODEL.init() };

        pr_info!("hackbot: loading module, creating /dev/hackbot\n");
        pr_info!(
            "hackbot: vLLM endpoint = {}.{}.{}.{}:{}\n",
            (VLLM_ADDR >> 24) & 0xFF,
            (VLLM_ADDR >> 16) & 0xFF,
            (VLLM_ADDR >> 8) & 0xFF,
            VLLM_ADDR & 0xFF,
            VLLM_PORT,
        );

        let options = MiscDeviceOptions {
            name: c_str!("hackbot"),
        };

        try_pin_init!(Self {
            _miscdev <- MiscDeviceRegistration::register(options),
        })
    }
}

#[pinned_drop]
impl PinnedDrop for HackbotModule {
    fn drop(self: Pin<&mut Self>) {
        free_model_resources();
        pr_info!("hackbot: unloading module\n");
    }
}

// ---------------------------------------------------------------------------
// Per-fd device state
// ---------------------------------------------------------------------------

#[pin_data(PinnedDrop)]
pub(crate) struct HackbotDev {
    dev: ARef<Device>,
}

#[vtable]
impl MiscDevice for HackbotDev {
    type Ptr = Pin<KBox<Self>>;

    fn open(_file: &File, misc: &MiscDeviceRegistration<Self>) -> Result<Pin<KBox<Self>>> {
        let dev = ARef::from(misc.device());
        dev_info!(dev, "hackbot: device opened\n");

        load_model_if_needed(&dev);

        KBox::try_pin_init(
            try_pin_init! {
                HackbotDev {
                    dev: dev,
                }
            },
            GFP_KERNEL,
        )
    }

    fn write_iter(_kiocb: Kiocb<'_, Self::Ptr>, iov: &mut IovIterSource<'_>) -> Result<usize> {
        let mut prompt = KVVec::new();
        let len = iov.copy_from_iter_vec(&mut prompt, GFP_KERNEL)?;

        if len == 0 {
            return Ok(0);
        }

        let response = process_prompt(&prompt);

        let mut guard = RESPONSE.lock();
        let copy_len = response.len().min(MAX_RESPONSE_SIZE);
        guard.data[..copy_len].copy_from_slice(&response[..copy_len]);
        guard.len = copy_len;
        guard.ready = true;
        drop(guard);

        Ok(len)
    }

    fn read_iter(mut kiocb: Kiocb<'_, Self::Ptr>, iov: &mut IovIterDest<'_>) -> Result<usize> {
        let guard = RESPONSE.lock();

        if !guard.ready {
            return Ok(0);
        }

        let data = &guard.data[..guard.len];
        let read = iov.simple_read_from_buffer(kiocb.ki_pos_mut(), data)?;

        Ok(read)
    }
}

#[pinned_drop]
impl PinnedDrop for HackbotDev {
    fn drop(self: Pin<&mut Self>) {
        dev_info!(self.dev, "hackbot: device closed\n");
    }
}
