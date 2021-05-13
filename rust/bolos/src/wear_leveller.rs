//! This module contains a struct to handle wear levelling for flash memory
use crate::{nvm::NVMError, NVM, PIC};

pub const PAGE_SIZE: usize = 64;
const COUNTER_SIZE: usize = std::mem::size_of::<u64>();
const CRC_SIZE: usize = std::mem::size_of::<u32>();

pub const SLOT_SIZE: usize = PAGE_SIZE - COUNTER_SIZE - CRC_SIZE;

pub const ZEROED_STORAGE: [u8; PAGE_SIZE] = Slot::zeroed().as_storage();

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

    pub const fn zeroed() -> Slot<'static> {
        const PAYLOAD_ZERO: [u8; SLOT_SIZE] = [0; SLOT_SIZE];
        const CRC_ZERO: u32 = 0x4128908;

        Slot {
            counter: 0,
            payload: &PAYLOAD_ZERO,
            crc: CRC_ZERO,
        }
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
        let payload = unsafe { &*(payload.as_ptr() as *const [u8; SLOT_SIZE]) };

        let crc = {
            let mut array = [0; CRC_SIZE];
            array.copy_from_slice(&storage[COUNTER_SIZE + SLOT_SIZE..]);
            u32::from_be_bytes(array)
        };

        let expected = Self::crc32(cnt, payload);
        if crc != expected {
            Err(SlotError::CRC {
                expected,
                found: crc,
            })?;
        }

        Ok(Slot {
            counter: cnt,
            crc,
            payload,
        })
    }

    pub const fn as_storage(&self) -> [u8; PAGE_SIZE] {
        let counter = self.counter.to_be_bytes();
        let crc = self.crc.to_be_bytes();

        let mut storage = [0; PAGE_SIZE];

        //storage[..COUNTER_SIZE].copy_from_slice(&counter);
        {
            let mut i = 0;
            while i < COUNTER_SIZE {
                storage[i] = counter[i];
                i += 1;
            }
        }
        //storage[COUNTER_SIZE..COUNTER_SIZE + SLOT_SIZE].copy_from_slice(&self.payload[..])
        {
            let mut i = 0;
            while i < SLOT_SIZE {
                storage[COUNTER_SIZE + i] = self.payload[i];
                i += 1;
            }
        }
        //storage[COUNTER_SIZE + SLOT_SIZE..].copy_from_slice(&crc);
        {
            let mut i = 0;
            while i < CRC_SIZE {
                storage[COUNTER_SIZE + SLOT_SIZE + i] = crc[i];
                i += 1;
            }
        }

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

#[derive(Debug, Copy, Clone)]
#[repr(transparent)]
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
            storage: NVM::zeroed(),
        }
    }

    pub fn with_baking<'m, const ARRAY_SIZE: usize, const BYTES: usize>(
        storage: &'m mut PIC<NVM<BYTES>>,
    ) -> &'m mut PIC<[NVMWearSlot; ARRAY_SIZE]> {
        //we need to make sure we passed the right details
        assert_eq!(BYTES, 64 * ARRAY_SIZE);

        let storage: *mut _ = storage;
        //Safety: this is ok because the memory layout is the same, since PIC is transparent
        // as well as NVMWearSlot
        unsafe {
            storage
                .cast::<PIC<[NVMWearSlot; ARRAY_SIZE]>>()
                .as_mut()
                .unwrap()
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

#[derive(Debug)]
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
    fn inc_idx(idx: usize) -> usize {
        (idx + 1) % S
    }

    fn inc(&mut self) {
        *self.idx = Self::inc_idx(*self.idx)
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
        //temporary index in case writing fails
        let idx = Self::inc_idx(*self.idx);

        let slot = &mut self.slots.get_mut()[idx];
        slot.write(payload)?;

        //now we can increment for sure because the slot was written
        self.inc();
        Ok(())
    }

    /// Retrieves the last written slot, which should also be the oldest
    pub fn read(&self) -> Result<&[u8; SLOT_SIZE], WearError> {
        //will only return CRC error
        self.slots.get_ref()[*self.idx].read()
    }
}

#[cfg(test)]
impl<'s, 'm, const S: usize> Wear<'s, 'm, S> {
    pub fn idx(&mut self) -> &mut usize {
        &mut *self.idx
    }

    pub fn slots(&mut self) -> &mut [NVMWearSlot; S] {
        &mut *self.slots.get_mut()
    }
}

#[macro_export]
macro_rules! new_wear_leveller {
    ($slots:expr) => {{
        const SLOTS: usize = $slots;
        const PAGE_SIZE: usize = $crate::wear_leveller::PAGE_SIZE;
        const BYTES: usize = SLOTS * PAGE_SIZE;

        #[$crate::nvm]
        static mut __BAKING_STORAGE: [[u8; PAGE_SIZE]; SLOTS] =
            $crate::wear_leveller::ZEROED_STORAGE;

        #[$crate::pic]
        static mut __IDX: usize = 0;

        unsafe {
            $crate::wear_leveller::Wear::new(
                $crate::wear_leveller::NVMWearSlot::with_baking::<$slots, BYTES>(
                    &mut __BAKING_STORAGE,
                ),
                &mut __IDX,
            )
        }
    }};
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macro_works() {
        let mut wear = new_wear_leveller!(2).expect("no nvm/crc issues");

        assert_eq!(0, *wear.idx());
        assert_eq!(2, wear.slots().len())
    }

    #[test]
    fn idx_increase() {
        let mut wear = new_wear_leveller!(5).expect("no nvm/crc issues");

        wear.write([42; SLOT_SIZE]).expect("no nvm issues");
        assert_eq!(1, *wear.idx());
    }

    #[test]
    fn idx_loop() {
        let mut wear = new_wear_leveller!(2).expect("no nvm/crc issues");
        assert_eq!(0, *wear.idx());

        wear.write([42; SLOT_SIZE]).expect("no nvm issues");
        assert_eq!(1, *wear.idx());
        wear.write([24; SLOT_SIZE]).expect("no nvm issues");
        assert_eq!(0, *wear.idx());
    }

    #[test]
    fn read_back() {
        let mut wear = new_wear_leveller!(1).expect("no nvm/crc issues");

        const MSG: [u8; SLOT_SIZE] = [42; SLOT_SIZE];

        wear.write(MSG).expect("no nvm issues");
        assert_eq!(&MSG, wear.read().expect("no nvm/crc issues"));
    }
}
