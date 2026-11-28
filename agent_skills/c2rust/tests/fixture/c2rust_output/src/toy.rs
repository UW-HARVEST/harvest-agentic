use ::c2rust_bitfields;
extern "C" {
    pub type _IO_wide_data;
    pub type _IO_codecvt;
    pub type _IO_marker;
    fn sqrt(__x: ::core::ffi::c_double) -> ::core::ffi::c_double;
    fn atoi(__nptr: *const ::core::ffi::c_char) -> ::core::ffi::c_int;
    fn strchr(__s: *const ::core::ffi::c_char, __c: ::core::ffi::c_int)
        -> *mut ::core::ffi::c_char;
    fn fprintf(
        __stream: *mut FILE,
        __format: *const ::core::ffi::c_char,
        ...
    ) -> ::core::ffi::c_int;
}
pub type __builtin_va_list = [__va_list_tag; 1];
#[derive(Copy, Clone)]
#[repr(C)]
pub struct __va_list_tag {
    pub gp_offset: ::core::ffi::c_uint,
    pub fp_offset: ::core::ffi::c_uint,
    pub overflow_arg_area: *mut ::core::ffi::c_void,
    pub reg_save_area: *mut ::core::ffi::c_void,
}
pub type va_list = __builtin_va_list;
pub type __uint64_t = u64;
pub type __off_t = ::core::ffi::c_long;
pub type __off64_t = ::core::ffi::c_long;
#[derive(Copy, Clone, BitfieldStruct)]
#[repr(C)]
pub struct _IO_FILE {
    pub _flags: ::core::ffi::c_int,
    pub _IO_read_ptr: *mut ::core::ffi::c_char,
    pub _IO_read_end: *mut ::core::ffi::c_char,
    pub _IO_read_base: *mut ::core::ffi::c_char,
    pub _IO_write_base: *mut ::core::ffi::c_char,
    pub _IO_write_ptr: *mut ::core::ffi::c_char,
    pub _IO_write_end: *mut ::core::ffi::c_char,
    pub _IO_buf_base: *mut ::core::ffi::c_char,
    pub _IO_buf_end: *mut ::core::ffi::c_char,
    pub _IO_save_base: *mut ::core::ffi::c_char,
    pub _IO_backup_base: *mut ::core::ffi::c_char,
    pub _IO_save_end: *mut ::core::ffi::c_char,
    pub _markers: *mut _IO_marker,
    pub _chain: *mut _IO_FILE,
    pub _fileno: ::core::ffi::c_int,
    #[bitfield(name = "_flags2", ty = "::core::ffi::c_int", bits = "0..=23")]
    pub _flags2: [u8; 3],
    pub _short_backupbuf: [::core::ffi::c_char; 1],
    pub _old_offset: __off_t,
    pub _cur_column: ::core::ffi::c_ushort,
    pub _vtable_offset: ::core::ffi::c_schar,
    pub _shortbuf: [::core::ffi::c_char; 1],
    pub _lock: *mut ::core::ffi::c_void,
    pub _offset: __off64_t,
    pub _codecvt: *mut _IO_codecvt,
    pub _wide_data: *mut _IO_wide_data,
    pub _freeres_list: *mut _IO_FILE,
    pub _freeres_buf: *mut ::core::ffi::c_void,
    pub _prevchain: *mut *mut _IO_FILE,
    pub _mode: ::core::ffi::c_int,
    pub _unused3: ::core::ffi::c_int,
    pub _total_written: __uint64_t,
    pub _unused2: [::core::ffi::c_char; 8],
}
pub type _IO_lock_t = ();
pub type FILE = _IO_FILE;
unsafe extern "C" fn sum_list(
    mut count: ::core::ffi::c_int,
    mut args: ::core::ffi::VaList,
) -> ::core::ffi::c_int {
    let mut sum: ::core::ffi::c_int = TOY_BIAS;
    let mut i: ::core::ffi::c_int = 0 as ::core::ffi::c_int;
    while i < count {
        sum += args.arg::<::core::ffi::c_int>();
        i += 1;
    }
    return sum;
}
#[no_mangle]
pub unsafe extern "C" fn toy_sum(
    mut count: ::core::ffi::c_int,
    mut args: ...
) -> ::core::ffi::c_int {
    let mut args_0: ::core::ffi::VaListImpl;
    args_0 = args.clone();
    let mut sum: ::core::ffi::c_int = sum_list(count, args_0.as_va_list());
    return sum;
}
#[no_mangle]
pub unsafe extern "C" fn toy_root(mut text: *const ::core::ffi::c_char) -> ::core::ffi::c_int {
    let mut colon: *const ::core::ffi::c_char = strchr(text, ':' as i32);
    if colon.is_null() {
        return -(1 as ::core::ffi::c_int);
    }
    return sqrt(atoi(text) as ::core::ffi::c_double) as ::core::ffi::c_int;
}
#[no_mangle]
pub unsafe extern "C" fn toy_print(
    mut out: *mut FILE,
    mut name: *const ::core::ffi::c_char,
    mut value: ::core::ffi::c_int,
) -> ::core::ffi::c_int {
    return fprintf(
        out,
        b"%s=%d\n\0" as *const u8 as *const ::core::ffi::c_char,
        name,
        value,
    );
}
pub const NULL: *mut ::core::ffi::c_void = ::core::ptr::null_mut::<::core::ffi::c_void>();
pub const TOY_BIAS: ::core::ffi::c_int = 1 as ::core::ffi::c_int;
