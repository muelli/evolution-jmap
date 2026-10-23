// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! What `ObjectData<T>` promises, measured on a plain `GObject`.

use std::cell::RefCell;
use std::ptr;
use std::rc::Rc;

use gobject_sys::{G_TYPE_OBJECT, GObject, g_object_new_with_properties, g_object_unref};
use jmap_backend_core::object_data::ObjectData;

/// A fresh `GObject` with one reference, which the caller owns.
fn plain_object() -> *mut GObject {
    // SAFETY: `G_TYPE_OBJECT` is instantiable and has no construct
    // properties, so zero properties with NULL arrays is a complete call.
    let object =
        unsafe { g_object_new_with_properties(G_TYPE_OBJECT, 0, ptr::null_mut(), ptr::null()) };
    assert!(
        !object.is_null(),
        "g_object_new_with_properties returned NULL"
    );
    object
}

/// Sets a flag when it is dropped.
struct DropFlag(Rc<RefCell<bool>>);

impl Drop for DropFlag {
    fn drop(&mut self) {
        *self.0.borrow_mut() = true;
    }
}

const NUMBER: ObjectData<RefCell<u32>> = ObjectData::new(c"jmap-test-object-data-number");
const FLAG: ObjectData<DropFlag> = ObjectData::new(c"jmap-test-object-data-flag");

#[test]
fn an_object_without_the_data_gives_none() {
    let object = plain_object();
    // SAFETY: `object` is live, and nothing set this key on it.
    assert!(unsafe { NUMBER.get(object) }.is_none());
    // SAFETY: the reference from `plain_object`.
    unsafe { g_object_unref(object) };
}

#[test]
fn get_gives_the_value_that_set_stored() {
    let object = plain_object();
    // SAFETY: `object` is live.
    unsafe { NUMBER.set(object, RefCell::new(7)) };
    // SAFETY: `object` is live, and `set` stored this type.
    let number = unsafe { NUMBER.get(object) }.expect("the data is set");
    *number.borrow_mut() += 1;
    // SAFETY: as above.
    assert_eq!(*unsafe { NUMBER.get(object) }.unwrap().borrow(), 8);
    // SAFETY: the reference from `plain_object`.
    unsafe { g_object_unref(object) };
}

#[test]
fn finalization_drops_the_value() {
    let dropped = Rc::new(RefCell::new(false));
    let object = plain_object();
    // SAFETY: `object` is live.
    unsafe { FLAG.set(object, DropFlag(Rc::clone(&dropped))) };
    assert!(!*dropped.borrow(), "the value is alive while the object is");
    // SAFETY: the reference from `plain_object`, the last one.
    unsafe { g_object_unref(object) };
    assert!(*dropped.borrow(), "finalization drops the value");
}

#[test]
fn a_second_set_drops_the_first_value() {
    let first = Rc::new(RefCell::new(false));
    let second = Rc::new(RefCell::new(false));
    let object = plain_object();
    // SAFETY: `object` is live.
    unsafe {
        FLAG.set(object, DropFlag(Rc::clone(&first)));
        FLAG.set(object, DropFlag(Rc::clone(&second)));
    }
    assert!(*first.borrow(), "the replaced value is dropped");
    assert!(!*second.borrow(), "the new value is kept");
    // SAFETY: the reference from `plain_object`, the last one.
    unsafe { g_object_unref(object) };
    assert!(*second.borrow());
}
