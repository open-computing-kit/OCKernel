use alloc::{boxed::Box, vec, vec::Vec};
use bitmask_enum::bitmask;
use core::{ffi::c_void, pin::Pin};
use x86::{
    dtables::{lidt, DescriptorTablePointer},
    io::outb,
};

use crate::FormatHex;

/// IDT flags
#[bitmask(u8)]
pub enum IDTFlags {
    Interrupt16 = 0x06,
    Trap16 = 0x07,
    Task32 = 0x05,
    Interrupt32 = 0x0e,
    Trap32 = 0x0f,
    Ring1 = 0x40,
    Ring2 = 0x20,
    Ring3 = 0x60,
    Present = 0x80,

    Interrupt = Self::Interrupt32.bits | Self::Present.bits,               // exception/interrupt
    Call = Self::Interrupt32.bits | Self::Present.bits | Self::Ring3.bits, // system call
}

/// entry in IDT
/// this describes an interrupt handler (i.e. where it is, how it works, etc)
#[repr(C, packed(16))]
#[derive(Copy, Clone, Debug)]
pub struct IDTEntry {
    /// low 16 bits of handler pointer
    isr_low: u16,

    /// GDT segment selector to be loaded before calling handler
    kernel_cs: u16,

    /// unused
    reserved: u8,

    /// type and attributes
    attributes: u8,

    /// high 16 bits of handler pointer
    isr_high: u16,
}

impl IDTEntry {
    /// creates a new IDT entry
    pub fn new(isr: *const (), flags: IDTFlags) -> Self {
        Self {
            // not sure if casting to u16 will only return lower 2 bytes?
            isr_low: ((isr as u32) & 0xffff) as u16, // gets address of function pointer, then chops off the top 2 bytes
            isr_high: ((isr as u32) >> 16) as u16,   // upper 2 bytes
            kernel_cs: 0x08,                         // offset of kernel code selector in GDT (see boot.S)
            attributes: flags.bits,
            reserved: 0,
        }
    }

    /// creates an empty IDT entry
    const fn new_empty() -> Self {
        Self {
            // empty entry
            isr_low: 0,
            kernel_cs: 0,
            reserved: 0,
            attributes: 0,
            isr_high: 0,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.isr_low == 0 && self.isr_high == 0
    }
}

pub fn init_pic() {
    unsafe {
        // reset PICs
        outb(0x20, 0x11);
        outb(0xa0, 0x11);

        // map primary PIC to interrupt 0x20-0x27
        outb(0x21, 0x20);

        // map secondary PIC to interrupt 0x28-0x2f
        outb(0xa1, 0x28);

        // set up cascading
        outb(0x21, 0x04);
        outb(0xa1, 0x02);

        outb(0x21, 0x01);
        outb(0xa1, 0x01);

        outb(0x21, 0x0);
        outb(0xa1, 0x0);
    }
}

#[repr(C, align(2))]
pub struct IDT {
    pub entries: [IDTEntry; 256],
}

impl IDT {
    pub fn new() -> Self {
        Self {
            entries: [IDTEntry::new_empty(); 256],
        }
    }

    /// # Safety
    ///
    /// this IDT must not be moved in memory at all or deallocated while it's loaded, otherwise undefined behavior will be caused
    pub unsafe fn load(&self) {
        lidt(&DescriptorTablePointer::new(&self.entries));
    }
}

impl Default for IDT {
    fn default() -> Self {
        Self::new()
    }
}

/// list of exceptions
pub enum Exceptions {
    /// divide-by-zero error
    DivideByZero = 0,

    /// debug
    Debug = 1,

    /// non-maskable interrupt
    NonMaskableInterrupt = 2,

    /// breakpoint
    Breakpoint = 3,

    /// overflow
    Overflow = 4,

    /// bound range exceeded
    BoundRangeExceeded = 5,

    /// invalid opcode
    InvalidOpcode = 6,

    /// device not available
    DeviceNotAvailable = 7,

    /// double fault
    DoubleFault = 8,

    /// coprocessor segment overrun
    CoprocessorSegmentOverrun = 9,

    /// invalid TSS
    InvalidTSS = 10,

    /// segment not present
    SegmentNotPresent = 11,

    /// stack segment fault
    StackSegmentFault = 12,

    /// general protection fault
    GeneralProtectionFault = 13,

    /// page fault
    PageFault = 14,

    /// x87 floating point exception
    FloatingPoint = 16,

    /// alignment check
    AlignmentCheck = 17,

    /// machine check
    MachineCheck = 18,

    /// SIMD floating point exception
    SIMDFloatingPoint = 19,

    /// virtualization exception
    Virtualization = 20,

    /// control protection exception
    ControlProtection = 21,

    /// hypervisor injection exception
    HypervisorInjection = 28,

    /// vmm communication exception
    VMMCommunication = 29,

    /// security exception
    Security = 30,
}

/// page fault error code wrapper
#[repr(transparent)]
pub struct PageFaultErrorCode(u32);

impl core::fmt::Display for PageFaultErrorCode {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "PageFaultErrorCode {{")?;

        if self.0 & (1 << 0) > 0 {
            write!(f, " present,")?;
        }

        if self.0 & (1 << 1) > 0 {
            write!(f, " write")?;
        } else {
            write!(f, " read")?;
        }

        if self.0 & (1 << 2) > 0 {
            write!(f, ", user mode")?;
        } else {
            write!(f, ", supervisor mode")?;
        }

        if self.0 & (1 << 3) > 0 {
            write!(f, ", reserved")?;
        }

        if self.0 & (1 << 4) > 0 {
            write!(f, ", instruction fetch")?;
        } else {
            write!(f, ", data access")?;
        }

        if self.0 & (1 << 5) > 0 {
            write!(f, ", protection-key")?;
        }

        if self.0 & (1 << 6) > 0 {
            write!(f, ", shadow")?;
        }

        if self.0 & (1 << 15) > 0 {
            write!(f, ", sgx")?;
        }

        write!(f, " }}")
    }
}

pub struct InterruptManager {
    idt: Pin<Box<IDT>>,
    data: Vec<Option<Interrupt>>,
}

impl InterruptManager {
    pub fn new() -> Self {
        let mut data = Vec::with_capacity(256);
        for _i in 0..256 {
            data.push(None);
        }

        Self { idt: Box::pin(IDT::new()), data }
    }

    pub fn register_interrupt<F: FnMut(&mut InterruptRegisters) + 'static>(&mut self, num: usize, handler: F) {
        let has_error_code = matches!(num, 8 | 10..=14 | 17 | 21 | 29 | 30);
        let data = Interrupt::new(handler, has_error_code);
        self.idt.entries[num] = IDTEntry::new(data.trampoline_ptr() as *const (), IDTFlags::Interrupt);
        self.data[num] = Some(data);
    }

    pub fn load_idt(&self) {
        unsafe {
            self.idt.load();
        }
    }
}

impl Default for InterruptManager {
    fn default() -> Self {
        Self::new()
    }
}

#[repr(C, packed(32))]
#[derive(Default, Copy, Clone)]
pub struct InterruptRegisters {
    pub ds: u32,
    pub edi: u32,
    pub esi: u32,
    pub ebp: u32,
    pub handler_esp: u32,
    pub ebx: u32,
    pub edx: u32,
    pub ecx: u32,
    pub eax: u32,
    pub error_code: u32,
    pub eip: u32,
    pub cs: u32,
    pub eflags: u32,
    pub esp: u32,
    pub ss: u32,
}

impl core::fmt::Debug for InterruptRegisters {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("InterruptRegisters")
            .field("ds", &FormatHex(self.ds))
            .field("edi", &FormatHex(self.edi))
            .field("esi", &FormatHex(self.esi))
            .field("ebp", &FormatHex(self.ebp))
            .field("handler_esp", &FormatHex(self.handler_esp))
            .field("ebx", &FormatHex(self.ebx))
            .field("edx", &FormatHex(self.edx))
            .field("ecx", &FormatHex(self.ecx))
            .field("eax", &FormatHex(self.eax))
            .field("error_code", &FormatHex(self.error_code))
            .field("eip", &FormatHex(self.eip))
            .field("cs", &FormatHex(self.cs))
            .field("eflags", &FormatHex(self.eflags))
            .field("esp", &FormatHex(self.esp))
            .field("ss", &FormatHex(self.ss))
            .finish()
    }
}

/// stores the trampoline code and data for an interrupt handler
#[allow(clippy::type_complexity)]
struct Interrupt {
    _handler: Pin<Box<dyn FnMut(&mut InterruptRegisters)>>,
    trampoline: Pin<Box<[u8]>>,
}

impl Interrupt {
    fn new<F: FnMut(&mut InterruptRegisters) + 'static>(handler: F, has_error_code: bool) -> Self {
        let handler = Box::pin(handler);

        let trampoline_data = (&*handler as *const _ as u32).to_ne_bytes();
        let trampoline_addr = (trampoline::<F> as *const () as u32).to_ne_bytes();

        #[rustfmt::skip]
        let mut handler_trampoline = vec![
            0x60,                           // pusha
            0x66, 0x8c, 0xd8,               // mov    ax,ds
            0x50,                           // push   eax
            0x66, 0xb8, 0x10, 0x00,         // mov    ax,0x10
            0x8e, 0xd8,                     // mov    ds,eax
            0x8e, 0xc0,                     // mov    es,eax
            0x8e, 0xe0,                     // mov    fs,eax
            0x8e, 0xe8,                     // mov    gs,eax
            0x54,                           // push   esp
            0xb8, trampoline_data[0], trampoline_data[1], trampoline_data[2], trampoline_data[3],   // mov    eax,<data>
            0x50,                           // push   eax
            0xb8, trampoline_addr[0], trampoline_addr[1], trampoline_addr[2], trampoline_addr[3],   // mov    eax,<addr>
            0xff, 0xd0,                     // call   eax
            0x83, 0xc4, 0x08,               // add    esp,0x8
            0x5b,                           // pop    ebx
            0x8e, 0xdb,                     // mov    ds,ebx
            0x8e, 0xc3,                     // mov    es,ebx
            0x8e, 0xe3,                     // mov    fs,ebx
            0x8e, 0xeb,                     // mov    gs,ebx
            0x61,                           // popa
            0x83, 0xc4, 0x04,               // add    esp,0x4
            0xcf,                           // iret 
        ];

        let handler_trampoline = if !has_error_code {
            let mut trampoline2 = vec![
                0x6a, 0x00, // push   0x0
            ];

            trampoline2.append(&mut handler_trampoline);

            trampoline2
        } else {
            handler_trampoline
        };

        Self {
            _handler: handler,
            trampoline: Box::into_pin(handler_trampoline.into_boxed_slice()),
        }
    }

    fn trampoline_ptr(&self) -> *const u8 {
        self.trampoline.as_ptr()
    }
}

// https://adventures.michaelfbryan.com/posts/rust-closures-in-ffi/
unsafe extern "C" fn trampoline<F: FnMut(&mut InterruptRegisters)>(data: *mut c_void, regs: &mut InterruptRegisters) {
    let data = &mut *(data as *mut F);
    data(regs);
}
