use crate::soc::types::Soc as SocT;
pub use apdu_dispatch::{
    command::SIZE as ApduCommandSize, response::SIZE as ApduResponseSize, App as ApduApp,
};
use apps::{Dispatch, Reboot, Variant};
use cortex_m::interrupt::InterruptNumber;
pub use ctaphid_dispatch::app::App as CtaphidApp;
#[cfg(feature = "se050")]
use embedded_hal::blocking::delay::DelayUs;
use embedded_time::duration::Milliseconds;
use littlefs2::{const_ram_storage, fs::Allocation, fs::Filesystem};
use nfc_device::traits::nfc::Device as NfcDevice;
use rand_chacha::ChaCha8Rng;
use trussed::{
    store,
    types::{LfsResult, LfsStorage},
    Platform,
};
use usb_device::bus::UsbBus;

pub mod usbnfc;

pub struct Config {
    pub card_issuer: &'static [u8; 13],
    pub usb_product: &'static str,
    pub usb_manufacturer: &'static str,
    // pub usb_release: u16 --> taken from utils::VERSION::usb_release()
    pub usb_id_vendor: u16,
    pub usb_id_product: u16,
}

pub const INTERFACE_CONFIG: Config = Config {
    // zero-padding for compatibility with previous implementations
    card_issuer: b"Nitrokey\0\0\0\0\0",
    usb_product: "Nitrokey 3",
    usb_manufacturer: "Nitrokey",
    usb_id_vendor: 0x20A0,
    usb_id_product: 0x42B2,
};

pub type Uuid = [u8; 16];

pub trait Soc: Reboot {
    type InternalFlashStorage;
    type ExternalFlashStorage;
    // VolatileStorage is always RAM
    type UsbBus: UsbBus + 'static;
    type NfcDevice: NfcDevice;
    type TrussedUI;

    #[cfg(feature = "se050")]
    type Se050Timer: DelayUs<u32>;
    #[cfg(feature = "se050")]
    type Twi: se05x::t1::I2CForT1;
    #[cfg(not(feature = "se050"))]
    type Se050Timer;
    #[cfg(not(feature = "se050"))]
    type Twi;

    type Duration: From<Milliseconds>;

    type Interrupt: InterruptNumber;
    const SYSCALL_IRQ: Self::Interrupt;

    const SOC_NAME: &'static str;
    const BOARD_NAME: &'static str;
    const VARIANT: Variant;

    fn device_uuid() -> &'static Uuid;
}

pub struct Runner {
    pub is_efs_available: bool,
}

impl apps::Runner for Runner {
    type Syscall = RunnerSyscall;
    type Reboot = SocT;
    type Store = RunnerStore;
    #[cfg(feature = "provisioner")]
    type Filesystem = <SocT as Soc>::InternalFlashStorage;
    type Twi = <SocT as Soc>::Twi;
    type Se050Timer = <SocT as Soc>::Se050Timer;

    fn uuid(&self) -> [u8; 16] {
        *<SocT as Soc>::device_uuid()
    }

    fn is_efs_available(&self) -> bool {
        self.is_efs_available
    }
}

// 8KB of RAM
const_ram_storage!(
    name = VolatileStorage,
    trait = LfsStorage,
    erase_value = 0xff,
    read_size = 16,
    write_size = 256,
    cache_size_ty = littlefs2::consts::U256,
    // We use 256 instead of the default 512 to avoid loosing too much space to nearly empty blocks containing only folder metadata.
    block_size = 256,
    block_count = 8192/256,
    lookahead_size_ty = littlefs2::consts::U1,
    filename_max_plus_one_ty = littlefs2::consts::U256,
    path_max_plus_one_ty = littlefs2::consts::U256,
    result = LfsResult,
);

store!(
    RunnerStore,
    Internal: <SocT as Soc>::InternalFlashStorage,
    External: <SocT as Soc>::ExternalFlashStorage,
    Volatile: VolatileStorage
);

pub static mut INTERNAL_STORAGE: Option<<SocT as Soc>::InternalFlashStorage> = None;
pub static mut INTERNAL_FS_ALLOC: Option<Allocation<<SocT as Soc>::InternalFlashStorage>> = None;
pub static mut INTERNAL_FS: Option<Filesystem<<SocT as Soc>::InternalFlashStorage>> = None;
pub static mut EXTERNAL_STORAGE: Option<<SocT as Soc>::ExternalFlashStorage> = None;
pub static mut EXTERNAL_FS_ALLOC: Option<Allocation<<SocT as Soc>::ExternalFlashStorage>> = None;
pub static mut EXTERNAL_FS: Option<Filesystem<<SocT as Soc>::ExternalFlashStorage>> = None;
pub static mut VOLATILE_STORAGE: Option<VolatileStorage> = None;
pub static mut VOLATILE_FS_ALLOC: Option<Allocation<VolatileStorage>> = None;
pub static mut VOLATILE_FS: Option<Filesystem<VolatileStorage>> = None;

pub struct RunnerPlatform {
    pub rng: ChaCha8Rng,
    pub store: RunnerStore,
    pub user_interface: <SocT as Soc>::TrussedUI,
}

unsafe impl Platform for RunnerPlatform {
    type R = ChaCha8Rng;
    type S = RunnerStore;
    type UI = <SocT as Soc>::TrussedUI;

    fn user_interface(&mut self) -> &mut Self::UI {
        &mut self.user_interface
    }

    fn rng(&mut self) -> &mut Self::R {
        &mut self.rng
    }

    fn store(&self) -> Self::S {
        self.store
    }
}

#[derive(Default)]
pub struct RunnerSyscall {}

impl trussed::client::Syscall for RunnerSyscall {
    #[inline]
    fn syscall(&mut self) {
        rtic::pend(<SocT as Soc>::SYSCALL_IRQ);
    }
}

pub type Trussed =
    trussed::Service<RunnerPlatform, Dispatch<<SocT as Soc>::Twi, <SocT as Soc>::Se050Timer>>;

pub type ApduDispatch = apdu_dispatch::dispatch::ApduDispatch<'static>;
pub type CtaphidDispatch = ctaphid_dispatch::dispatch::Dispatch<'static, 'static>;

pub type Apps = apps::Apps<Runner>;

#[derive(Debug)]
pub struct DelogFlusher {}

impl delog::Flusher for DelogFlusher {
    fn flush(&self, _msg: &str) {
        #[cfg(feature = "log-rtt")]
        rtt_target::rprint!(_msg);

        #[cfg(feature = "log-semihosting")]
        cortex_m_semihosting::hprint!(_msg).ok();

        // TODO: re-enable?
        // #[cfg(feature = "log-serial")]
        // see https://git.io/JLARR for the plan on how to improve this once we switch to RTIC 0.6
        // rtic::pend(hal::raw::Interrupt::MAILBOX);
    }
}

pub static DELOG_FLUSHER: DelogFlusher = DelogFlusher {};
