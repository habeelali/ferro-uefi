//! A real, unmodified `aarch64-unknown-uefi` application -- built by
//! rustc's own UEFI target, not a hand-crafted PE file -- used to
//! verify Ferro's console I/O, file system, and boot services
//! implementations against actual EFI ABI calls a real bootloader
//! would make. No `uefi` crate dependency: the protocol structs below
//! are hand-written straight from the UEFI spec, exactly the way a
//! minimal C EFI application (or Ferro's own firmware code) would
//! define them, so this is testing Ferro's real ABI compatibility,
//! not agreement with some other Rust crate's assumptions.

#![no_std]
#![no_main]

use core::ffi::c_void;
use core::panic::PanicInfo;

type Status = usize;
type Handle = *mut c_void;

const EFI_SUCCESS: Status = 0;

#[repr(C)]
struct TableHeader {
    signature: u64,
    revision: u32,
    header_size: u32,
    crc32: u32,
    reserved: u32,
}

#[repr(C)]
struct SimpleTextOutput {
    reset: extern "efiapi" fn(*mut SimpleTextOutput, u8) -> Status,
    output_string: extern "efiapi" fn(*mut SimpleTextOutput, *const u16) -> Status,
    test_string: usize,
    query_mode: usize,
    set_mode: usize,
    set_attribute: usize,
    clear_screen: extern "efiapi" fn(*mut SimpleTextOutput) -> Status,
    set_cursor_position: usize,
    enable_cursor: usize,
    mode: *mut c_void,
}

#[repr(C)]
struct InputKey {
    scan_code: u16,
    unicode_char: u16,
}

#[repr(C)]
struct SimpleTextInput {
    reset: usize,
    read_key_stroke: extern "efiapi" fn(*mut SimpleTextInput, *mut InputKey) -> Status,
    wait_for_key: *mut c_void,
}

#[repr(C)]
struct Guid {
    d1: u32,
    d2: u16,
    d3: u16,
    d4: [u8; 8],
}

const SIMPLE_FILE_SYSTEM_GUID: Guid = Guid {
    d1: 0x964E_5B22,
    d2: 0x6459,
    d3: 0x11D2,
    d4: [0x8E, 0x39, 0x00, 0xA0, 0xC9, 0x69, 0x72, 0x3B],
};

// Fields this app never calls are kept as plain `usize` placeholders
// (same size/alignment as a function pointer on aarch64), not
// omitted -- omitting them would shift every field after into the
// wrong offset, which is exactly the kind of ABI bug this app exists
// to catch.
#[repr(C)]
struct BootServices {
    hdr: TableHeader,
    raise_tpl: usize,
    restore_tpl: usize,
    allocate_pages: usize,
    free_pages: usize,
    get_memory_map: usize,
    allocate_pool: usize,
    free_pool: usize,
    create_event: usize,
    set_timer: usize,
    wait_for_event: usize,
    signal_event: usize,
    close_event: usize,
    check_event: usize,
    install_protocol_interface: usize,
    reinstall_protocol_interface: usize,
    uninstall_protocol_interface: usize,
    handle_protocol: usize,
    reserved: usize,
    register_protocol_notify: usize,
    locate_handle: usize,
    locate_device_path: usize,
    install_configuration_table: usize,
    load_image: usize,
    start_image: usize,
    exit: usize,
    unload_image: usize,
    exit_boot_services: usize,
    get_next_monotonic_count: usize,
    stall: extern "efiapi" fn(usize) -> Status,
    set_watchdog_timer: usize,
    connect_controller: usize,
    disconnect_controller: usize,
    open_protocol: usize,
    close_protocol: usize,
    open_protocol_information: usize,
    protocols_per_handle: usize,
    locate_handle_buffer: usize,
    locate_protocol: extern "efiapi" fn(*const Guid, *mut c_void, *mut *mut c_void) -> Status,
    install_multiple_protocol_interfaces: usize,
    uninstall_multiple_protocol_interfaces: usize,
    calculate_crc32: usize,
    copy_mem: usize,
    set_mem: usize,
    create_event_ex: usize,
}

#[repr(C)]
struct SystemTable {
    hdr: TableHeader,
    firmware_vendor: *const u16,
    firmware_revision: u32,
    console_in_handle: Handle,
    con_in: *mut SimpleTextInput,
    console_out_handle: Handle,
    con_out: *mut SimpleTextOutput,
    standard_error_handle: Handle,
    std_err: *mut c_void,
    runtime_services: *mut c_void,
    boot_services: *mut BootServices,
    number_of_table_entries: usize,
    configuration_table: *mut c_void,
}

#[repr(C)]
struct FileProtocol {
    revision: u64,
    open: extern "efiapi" fn(*mut FileProtocol, *mut *mut FileProtocol, *const u16, u64, u64) -> Status,
    close: extern "efiapi" fn(*mut FileProtocol) -> Status,
    delete: usize,
    read: extern "efiapi" fn(*mut FileProtocol, *mut usize, *mut c_void) -> Status,
    write: usize,
    get_position: usize,
    set_position: usize,
    get_info: usize,
    set_info: usize,
    flush: usize,
}

#[repr(C)]
struct SimpleFileSystem {
    revision: u64,
    open_volume: extern "efiapi" fn(*mut SimpleFileSystem, *mut *mut FileProtocol) -> Status,
}

fn print(st: &SystemTable, s: &str) {
    let mut buf = [0u16; 256];
    let mut n = 0;
    for c in s.chars() {
        if n >= buf.len() - 1 {
            break;
        }
        buf[n] = c as u16;
        n += 1;
    }
    buf[n] = 0;
    ((unsafe { &*st.con_out }).output_string)(st.con_out, buf.as_ptr());
}

fn print_decimal(st: &SystemTable, mut n: u64) {
    let mut digits = [0u8; 20];
    let mut i = digits.len();
    if n == 0 {
        i -= 1;
        digits[i] = b'0';
    }
    while n > 0 {
        i -= 1;
        digits[i] = b'0' + (n % 10) as u8;
        n /= 10;
    }
    let len = digits.len() - i;
    let mut buf16 = [0u16; 21];
    for (j, &d) in digits[i..].iter().enumerate() {
        buf16[j] = d as u16;
    }
    buf16[len] = 0;
    ((unsafe { &*st.con_out }).output_string)(st.con_out, buf16.as_ptr());
}

const TEST_FILE_NAME: [u16; 9] = [
    'T' as u16, 'E' as u16, 'S' as u16, 'T' as u16, '.' as u16, 'T' as u16, 'X' as u16, 'T' as u16, 0,
];

const NESTED_FILE_PATH: [u16; 16] = [
    '\\' as u16, 'S' as u16, 'U' as u16, 'B' as u16, '\\' as u16, 'N' as u16, 'E' as u16, 'S' as u16, 'T' as u16,
    'E' as u16, 'D' as u16, '.' as u16, 'T' as u16, 'X' as u16, 'T' as u16, 0,
];

#[no_mangle]
extern "efiapi" fn efi_main(_image_handle: Handle, system_table: *mut SystemTable) -> Status {
    let st = unsafe { &*system_table };

    (unsafe { &*st.con_out }.clear_screen)(st.con_out);
    print(st, "==============================================\r\n");
    print(st, "  Ferro UEFI conformance test\r\n");
    print(st, "  (real aarch64-unknown-uefi binary, not hand-crafted)\r\n");
    print(st, "==============================================\r\n\r\n");
    print(st, "[1] ConOut.OutputString: if you can read this, it works.\r\n\r\n");

    let bs = unsafe { &*st.boot_services };

    print(st, "[2] LocateProtocol(EFI_SIMPLE_FILE_SYSTEM_PROTOCOL)... ");
    let mut sfs_iface: *mut c_void = core::ptr::null_mut();
    let status = (bs.locate_protocol)(&SIMPLE_FILE_SYSTEM_GUID, core::ptr::null_mut(), &mut sfs_iface);
    if status != EFI_SUCCESS || sfs_iface.is_null() {
        print(st, "FAILED (status=");
        print_decimal(st, status as u64);
        print(st, ")\r\n");
    } else {
        print(st, "OK\r\n");
        let sfs = sfs_iface as *mut SimpleFileSystem;

        print(st, "[3] OpenVolume... ");
        let mut root: *mut FileProtocol = core::ptr::null_mut();
        let status = (unsafe { &*sfs }.open_volume)(sfs, &mut root);
        if status != EFI_SUCCESS || root.is_null() {
            print(st, "FAILED\r\n");
        } else {
            print(st, "OK\r\n");

            print(st, "[4] Open(\"TEST.TXT\")... ");
            let mut file: *mut FileProtocol = core::ptr::null_mut();
            let status = (unsafe { &*root }.open)(root, &mut file, TEST_FILE_NAME.as_ptr(), 1, 0);
            if status != EFI_SUCCESS || file.is_null() {
                print(st, "FAILED (status=");
                print_decimal(st, status as u64);
                print(st, ") -- expected if TEST.TXT wasn't placed on the card\r\n");
            } else {
                print(st, "OK\r\n");

                print(st, "[5] Read()... ");
                let mut buf = [0u8; 128];
                let mut size = buf.len();
                let status = (unsafe { &*file }.read)(file, &mut size, buf.as_mut_ptr() as *mut c_void);
                if status != EFI_SUCCESS {
                    print(st, "FAILED\r\n");
                } else {
                    print(st, "OK, read ");
                    print_decimal(st, size as u64);
                    print(st, " bytes: \"");
                    let mut u16buf = [0u16; 128];
                    for i in 0..size {
                        u16buf[i] = buf[i] as u16;
                    }
                    let mut nul = [0u16; 129];
                    nul[..size].copy_from_slice(&u16buf[..size]);
                    nul[size] = 0;
                    (unsafe { &*st.con_out }.output_string)(st.con_out, nul.as_ptr());
                    print(st, "\"\r\n");
                }
                (unsafe { &*file }.close)(file);
            }

            print(st, "[6] Open(\"\\SUB\\NESTED.TXT\") -- subdirectory traversal... ");
            let mut nested: *mut FileProtocol = core::ptr::null_mut();
            let status = (unsafe { &*root }.open)(root, &mut nested, NESTED_FILE_PATH.as_ptr(), 1, 0);
            if status != EFI_SUCCESS || nested.is_null() {
                print(st, "FAILED (status=");
                print_decimal(st, status as u64);
                print(st, ") -- expected if SUB\\NESTED.TXT wasn't placed on the card\r\n");
            } else {
                print(st, "OK\r\n");
                print(st, "[7] Read() the nested file... ");
                let mut buf = [0u8; 128];
                let mut size = buf.len();
                let status = (unsafe { &*nested }.read)(nested, &mut size, buf.as_mut_ptr() as *mut c_void);
                if status != EFI_SUCCESS {
                    print(st, "FAILED\r\n");
                } else {
                    print(st, "OK, read ");
                    print_decimal(st, size as u64);
                    print(st, " bytes: \"");
                    let mut u16buf = [0u16; 129];
                    for i in 0..size {
                        u16buf[i] = buf[i] as u16;
                    }
                    u16buf[size] = 0;
                    (unsafe { &*st.con_out }.output_string)(st.con_out, u16buf.as_ptr());
                    print(st, "\"\r\n");
                }
                (unsafe { &*nested }.close)(nested);
            }
        }
    }

    print(st, "\r\n[8] Waiting for a keypress via ConIn.ReadKeyStroke...\r\n");
    let mut key = InputKey { scan_code: 0, unicode_char: 0 };
    loop {
        let status = (unsafe { &*st.con_in }.read_key_stroke)(st.con_in, &mut key);
        if status == EFI_SUCCESS {
            break;
        }
        (bs.stall)(50_000);
    }
    print(st, "    got scan_code=");
    print_decimal(st, key.scan_code as u64);
    print(st, " unicode_char=");
    print_decimal(st, key.unicode_char as u64);
    print(st, "\r\n\r\nAll checks done. Returning EFI_SUCCESS to the firmware.\r\n");

    EFI_SUCCESS
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {}
}
