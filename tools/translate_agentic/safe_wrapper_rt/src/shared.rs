//! Exported mutable globals and errno.

use std::ffi::c_int;
use std::sync::atomic::{self, Ordering};

/// An exported mutable global that C callers and the Rust code both read and write, for example
/// `extern int lib_debug_level;`. Define it in `wrap` as
/// `#[unsafe(no_mangle)] pub static lib_debug_level: CShared<c_int> = CShared::new(0);`.
///
/// The Rust side reads and writes it with relaxed atomic operations (plain loads and stores on
/// x86-64 and aarch64), so safe code cannot cause a data race. A race with a C caller is a race in C
/// too. When C promises thread-safe access with more than this, use the C rules instead (S11).
#[repr(transparent)]
pub struct CShared<T: SharedValue>(std::cell::UnsafeCell<T>);

// SAFETY: get and set are atomic, and no reference into the value goes out.
unsafe impl<T: SharedValue> Sync for CShared<T> {}

impl<T: SharedValue> CShared<T> {
    pub const fn new(v: T) -> Self {
        CShared(std::cell::UnsafeCell::new(v))
    }

    pub fn get(&self) -> T {
        // SAFETY: the address is valid and aligned for the atomic type, and every Rust access is atomic.
        unsafe { T::load(self.0.get()) }
    }

    pub fn set(&self, v: T) {
        // SAFETY: as in get.
        unsafe { T::store(self.0.get(), v) }
    }

    /// The address that goes to C.
    pub fn as_ptr(&self) -> *mut T {
        self.0.get()
    }
}

/// A value that [`CShared`] can hold: an integer or a raw pointer, which has an atomic type.
pub trait SharedValue: Copy {
    /// # Safety
    /// `p` is valid and aligned, and every concurrent access is atomic.
    unsafe fn load(p: *mut Self) -> Self;

    /// # Safety
    /// As in `load`.
    unsafe fn store(p: *mut Self, v: Self);
}

macro_rules! shared_value {
    ($($t:ty => $atomic:ty),*) => { $(
        impl SharedValue for $t {
            unsafe fn load(p: *mut Self) -> Self {
                // SAFETY: the caller's contract. The atomic type has the size and alignment of $t.
                unsafe { <$atomic>::from_ptr(p) }.load(Ordering::Relaxed)
            }

            unsafe fn store(p: *mut Self, v: Self) {
                // SAFETY: as in load.
                unsafe { <$atomic>::from_ptr(p) }.store(v, Ordering::Relaxed)
            }
        }
    )* };
}
shared_value!(
    i8 => atomic::AtomicI8, u8 => atomic::AtomicU8, i16 => atomic::AtomicI16,
    u16 => atomic::AtomicU16, i32 => atomic::AtomicI32, u32 => atomic::AtomicU32,
    i64 => atomic::AtomicI64, u64 => atomic::AtomicU64, isize => atomic::AtomicIsize,
    usize => atomic::AtomicUsize
);

impl<T> SharedValue for *mut T {
    unsafe fn load(p: *mut Self) -> Self {
        // SAFETY: the caller's contract. AtomicPtr has the size and alignment of a pointer.
        unsafe { atomic::AtomicPtr::from_ptr(p) }.load(Ordering::Relaxed)
    }

    unsafe fn store(p: *mut Self, v: Self) {
        // SAFETY: as in load.
        unsafe { atomic::AtomicPtr::from_ptr(p) }.store(v, Ordering::Relaxed)
    }
}

impl<T> SharedValue for *const T {
    unsafe fn load(p: *mut Self) -> Self {
        // SAFETY: as for *mut T.
        unsafe { <*mut T>::load(p.cast()) }.cast_const()
    }

    unsafe fn store(p: *mut Self, v: Self) {
        // SAFETY: as for *mut T.
        unsafe { <*mut T>::store(p.cast(), v.cast_mut()) }
    }
}

#[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
compile_error!("errno: add the errno location function of this target");

unsafe extern "C" {
    #[cfg_attr(target_os = "linux", link_name = "__errno_location")]
    #[cfg_attr(target_vendor = "apple", link_name = "__error")]
    fn errno_location() -> *mut c_int;
}

/// The C errno of this thread.
pub fn errno() -> c_int {
    // SAFETY: the C library gives a valid errno address for the calling thread.
    unsafe { errno_location().read() }
}

/// Sets the C errno of this thread, as C code does with `errno = v`.
pub fn set_errno(v: c_int) {
    // SAFETY: as in errno.
    unsafe { errno_location().write(v) }
}
