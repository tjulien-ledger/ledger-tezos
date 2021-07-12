/*******************************************************************************
*   (c) 2021 Zondax GmbH
*
*  Licensed under the Apache License, Version 2.0 (the "License");
*  you may not use this file except in compliance with the License.
*  You may obtain a copy of the License at
*
*      http://www.apache.org/licenses/LICENSE-2.0
*
*  Unless required by applicable law or agreed to in writing, software
*  distributed under the License is distributed on an "AS IS" BASIS,
*  WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
*  See the License for the specific language governing permissions and
*  limitations under the License.
********************************************************************************/
use super::UIBackend;
use crate::{
    ui::{manual_vtable::RefMutDynViewable, Viewable},
    ui_toolkit::{strlen, ZUI},
};

use arrayvec::ArrayString;

const KEY_SIZE: usize = 17 + 1;
//with null terminator
const MESSAGE_LINE_SIZE: usize = 17 + 1;
const MESSAGE_SIZE: usize = 2 * MESSAGE_LINE_SIZE + 1;

const INCLUDE_ACTIONS_AS_ITEMS: usize = 2;
const INCLUDE_ACTIONS_COUNT: usize = INCLUDE_ACTIONS_AS_ITEMS - 1;

#[bolos_derive::lazy_static]
pub static mut RUST_ZUI: ZUI<NanoSBackend, KEY_SIZE, MESSAGE_SIZE> = ZUI::new();

pub struct NanoSBackend {
    key: ArrayString<KEY_SIZE>,

    message_line1: ArrayString<MESSAGE_LINE_SIZE>,
    message_line2: ArrayString<MESSAGE_LINE_SIZE>,

    viewable_size: usize,
    expert: bool,
}

impl NanoSBackend {
    pub fn update_expert(&mut self) {
        self.message_line1.clear();
        self.message_line2.clear();

        let msg = if self.expert { "enabled" } else { "disabled" };

        write!(self.message_line1, "{}", msg).expect("unable to write expert");
    }


}

impl Default for NanoSBackend {
    fn default() -> Self {
        Self {
            key: ArrayString::new_const(),
            message_line1: ArrayString::new_const(),
            message_line2: ArrayString::new_const(),
            viewable_size: 0,
            expert: false,
        }
    }
}

impl UIBackend<KEY_SIZE, MESSAGE_SIZE> for NanoSBackend {
    const INCLUDE_ACTIONS_COUNT: usize = INCLUDE_ACTIONS_COUNT;

    fn key_buf(&mut self) -> &mut ArrayString<KEY_SIZE> {
        &mut self.key
    }

    fn message_buf(&self) -> ArrayString<MESSAGE_SIZE> {
        ArrayString::new_const()
    }

    fn split_value_field(&mut self, message_buf: ArrayString<MESSAGE_SIZE>) {
        use core::fmt::Write;

        //clear inner message buffers
        self.message_line1.clear();
        self.message_line2.clear();

        //compute len and split `message_buf` at the max line size or at the total len
        // if the total len is less than the size of 1 line
        let len = strlen(message_buf.as_bytes());
        let split = core::cmp::min(MESSAGE_LINE_SIZE, len);
        let (line1, line2) = message_buf.split_at(split);

        //write the 2 lines, so if the message was small enough to fit
        // on the first line
        // then the second line will stay empty
        write!(&mut self.message_line1, "{}", line1);
        write!(&mut self.message_line2, "{}", line2);
    }

    fn show_idle(&mut self, item_idx: u8, status: Option<&str>) {
        let status = status.unwrap_or("DO NOT USE"); //FIXME: MENU_MAIN_APP_LINE2

        self.key.clear();
        write!(self.key, "{}", status);

        self.update_expert();
        todo!("UX_MENU_DISPLAY(item_idx, menu_main, NULL)")
    }

    fn show_error(&mut self) {
        todo!("UX_DISPLAY(view_error, view_prepro)");
    }

    fn show_review(ui: &mut ZUI<Self, KEY_SIZE, MESSAGE_SIZE>) {
        //reset ui struct
        ui.paging_init();

        match ui.review_update_data() {
            Ok(_) => {
                todo!("UX_DISPLAY(view_review, view_prepro)")
            }
            Err(_) => ui.show_error(),
        }
    }

    fn update_review(ui: &mut ZUI<Self, KEY_SIZE, MESSAGE_SIZE>) {
        match ui.review_update_data() {
            Ok(_) => {
                todo!("UX_DISPLAY(view_review, view_prepro)")
            }
            Err(_) => {
                ui.show_error();
                ui.backend.wait_ui();
            }
        }
    }

    fn wait_ui(&mut self) {
        //FIXME: UX_WAIT
    }

    fn expert(&self) -> bool {
        self.expert
    }

    fn toggle_expert(&mut self) {
        self.expert = !self.expert;
    }

    fn accept_reject_out(&mut self) -> &mut [u8] {
        use bolos_sys::raw::G_io_apdu_buffer as APDU_BUFFER;

        unsafe { &mut APDU_BUFFER[..APDU_BUFFER.len() - self.viewable_size] }
    }

    fn accept_reject_end(&mut self, len: usize) {
        use bolos_sys::raw::{io_exchange, CHANNEL_APDU, IO_RETURN_AFTER_TX};

        io_exchange((CHANNEL_APDU | IO_RETURN_AFTER_TX) as u8, len as u16);
    }

    fn store_viewable<V: Viewable + Sized + 'static>(
        &mut self,
        viewable: V,
    ) -> Option<RefMutDynViewable> {
        use bolos_sys::raw::G_io_apdu_buffer as APDU_BUFFER;

        let size = core::mem::size_of::<V>();
        unsafe {
            let buf_len = APDU_BUFFER.len();
            if size > buf_len {
                return None;
            }

            let new_loc_slice = &mut APDU_BUFFER[buf_len - size..];
            let new_loc_raw_ptr: *mut u8 = new_loc_slice.as_mut_ptr();
            let new_loc: *mut V = new_loc_raw_ptr.cast();

            //write but we don't want to drop `new_loc` since
            // it's not actually valid T data
            core::ptr::write(new_loc, item);

            //write how many bytes we have occupied
            self.viewable_size = size;

            //we can unwrap as we know this ptr is valid
            Some(new_loc.as_mut().unwrap().into())
        }
    }
}

mod cabi {
    use super::*;

    #[no_mangle]
    pub unsafe extern "C" fn viewdata_key() -> *mut u8 {
        RUST_ZUI.backend.key.as_bytes_mut().as_mut_ptr()
    }

    #[no_mangle]
    pub unsafe extern "C" fn viewdata_message_line1() -> *mut u8 {
        RUST_ZUI.backend.message_line1.as_bytes_mut().as_mut_ptr()
    }

    #[no_mangle]
    pub unsafe extern "C" fn viewdata_message_line2() -> *mut u8 {
        RUST_ZUI.backend.message_line2.as_bytes_mut().as_mut_ptr()
    }

    #[no_mangle]
    pub unsafe extern "C" fn rs_h_expert_toggle() {
        RUST_ZUI.backend.toggle_expert()
    }

    #[no_mangle]
    pub unsafe extern "C" fn rs_h_paging_can_decrease() -> bool {
        RUST_ZUI.paging_can_decrease()
    }
}
