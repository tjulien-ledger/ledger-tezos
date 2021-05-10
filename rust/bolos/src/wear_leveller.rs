//! This module contains a struct to handle wear levelling for flash memory
use crate::{nvm::NVMError, NVM, PIC};

const PAGE_SIZE: usize = 64;
const COUNTER_SIZE: usize = std::mem::size_of::<u64>();
const CRC_SIZE: usize = std::mem::size_of::<u32>();

const SLOT_SIZE: usize = PAGE_SIZE - COUNTER_SIZE - CRC_SIZE;

#[derive(Debug)]
struct Slot<'nvm> {
    pub counter: u64,
    payload: &'nvm [u8; SLOT_SIZE],
    crc: u32,
}

#[derive(Debug)]
enum SlotError {
    CRC { expected: u32, found: u32 },
}

impl<'nvm> Slot<'nvm> {
    fn crc32(counter: u64, payload: &[u8; SLOT_SIZE]) -> u32 {
        use crc::crc32::*;

        let mut digest = Digest::new(IEEE);
        digest.write(&counter.to_be_bytes()[..]);
        digest.write(&payload[..]);

        digest.sum32()

    }

    pub fn from_storage(storage: &'nvm [u8; PAGE_SIZE]) -> Result<Self, SlotError> {
        let cnt = {
            let mut array = [0; COUNTER_SIZE];
            array.copy_from_slice(&storage[..COUNTER_SIZE]);
            u64::from_be_bytes(array)
        };

        //safety: this is safe because we are reinrepreting a reference so we uphold
        // borrow checker rules
        // also the size matches
        let payload = &storage[COUNTER_SIZE..COUNTER_SIZE + SLOT_SIZE];
        let payload = unsafe { &*(*payload.as_ptr() as *const [u8; SLOT_SIZE]) };

        let crc = {
            let mut array = [0; CRC_SIZE];
            array.copy_from_slice(&storage[COUNTER_SIZE + SLOT_SIZE..]);
            u32::from_be_bytes(array)
        };

        let expected = Self::crc32(cnt, payload);
        if crc != expected {
            Err(SlotError::CRC{expected, found: crc})?;
        }

        Ok(Slot {
            counter: cnt,
            crc,
            payload,
        })
    }

    pub fn as_storage(&self) -> [u8; PAGE_SIZE] {
        let counter = self.counter.to_be_bytes();
        let crc = self.crc.to_be_bytes();

        let mut storage = [0; PAGE_SIZE];
        storage[..COUNTER_SIZE].copy_from_slice(&counter);
        storage[COUNTER_SIZE..COUNTER_SIZE + SLOT_SIZE].copy_from_slice(&self.payload[..]);
        storage[COUNTER_SIZE + SLOT_SIZE..].copy_from_slice(&crc);

        storage
    }

    pub fn modify<'new>(&self, payload: &'new [u8; SLOT_SIZE]) -> Slot<'new> {
        let counter = self.counter + 1;
        let crc = Self::crc32(counter, payload);

        Slot {
            counter,
            crc,
            payload,
        }
    }
}

#[derive(Copy, Clone)]
pub struct NVMWearSlot {
    storage: NVM<64>,
}

#[derive(Debug)]
pub enum WearError {
    CRC { expected: u32, found: u32 },
    NVMWrite,
}

impl NVMWearSlot {
    pub const fn new() -> Self {
        Self {
            storage: NVM::new(),
        }
    }

    fn counter(&self) -> Option<u64> {
        self.as_slot().ok().map(|s| s.counter)
    }

    fn as_slot(&self) -> Result<Slot<'_>, WearError> {
        Slot::from_storage(&self.storage)
            .map_err(|SlotError::CRC { expected, found }| WearError::CRC { expected, found })
    }

    /// Reads the payload of the slot (if valid)
    pub fn read(&self) -> Result<&[u8; SLOT_SIZE], WearError> {
        Slot::from_storage(&self.storage)
            .map(|s| s.payload)
            .map_err(|SlotError::CRC { expected, found }| WearError::CRC { expected, found })
    }

    /// Write `slice` to the inner slot
    pub fn write(&mut self, write: [u8; SLOT_SIZE]) -> Result<(), WearError> {
        let storage = self.as_slot()?.modify(&write).as_storage();

        self.storage.write(0, &storage).map_err(|e| match e {
            NVMError::Write => WearError::NVMWrite,
            _ => unreachable!("size is checked already"),
        })
    }
}

pub struct Wear<'s, 'm, const SLOTS: usize> {
    slots: &'s mut PIC<[NVMWearSlot; SLOTS]>,
    idx: &'m mut usize,
}

impl<'s, 'm, const S: usize> Wear<'s, 'm, S> {
    pub fn new(
        slots: &'s mut PIC<[NVMWearSlot; S]>,
        idx: &'m mut usize,
    ) -> Result<Self, WearError> {
        let mut me = Self { slots, idx };
        *me.idx = me.align()?;

        Ok(me)
    }

    /// Increments idx staying in the bounds of the slot
    fn inc(&mut self) {
        *self.idx = (*self.idx + 1) % S;
    }

    /// Aligns `idx` to the correct position on the tape
    ///
    /// This is most useful when `slots` is not blank data
    fn align(&mut self) -> Result<usize, WearError> {
        let mut max = 0;
        let mut idx = 0;

        for (i, slot) in self.slots.iter().enumerate() {
            let cnt = slot.as_slot()?.counter;
            if cnt > max {
                max = cnt;
                idx = i;
            }
        }

        Ok(idx)
    }

    /// Retrieves the next slot to write, which should be also the youngest
    ///
    /// Will wrap when the end has been reached
    pub fn write(&mut self, payload: [u8; SLOT_SIZE]) -> Result<(), WearError> {
        let mut slot = self.slots.get_mut()[*self.idx];
        slot.write(payload)?;

        //the write checks already if we wrote succesfully
        self.inc();
        Ok(())
    }

    /// Retrieves the last written slot, which should also be the oldest
    pub fn read(&self) -> Result<&[u8; SLOT_SIZE], WearError> {
        //will only return CRC error
        self.slots[*self.idx].read()
    }
}

#[macro_export]
macro_rules! new_wear_leveller {
    ($slots:expr) => {{
        #[$crate::pic]
        static mut __SLOTS: [$crate::wear_leveller::NVMWearSlot; $slots] =
            [$crate::wear_leveller::NVMWearSlot::new(); $slots];

        #[$crate::pic]
        static mut __IDX: usize = 0;

        unsafe { $crate::wear_leveller::Wear::new(&mut __SLOTS, &mut __IDX) }
    }};
}
