#![no_std]
#![no_main]
// i dont understand why this is a default
// if im specifying groupings, then why do they assume i need it on byte boundaries
// i frequently want it on bit or even nibble boundaries
// and the clippy suggestion is to group them by nibbles too, so it's not even the right name
#![allow(clippy::unusual_byte_groupings)]

use panic_rtt_target as _;

use core::cell::RefCell;
use cortex_m::asm;
use cortex_m::peripheral::NVIC;
use critical_section::Mutex;
use rtt_target::{rprintln, rtt_init_print};
use stm32f3::stm32f303::interrupt;
use stm32f3::stm32f303::{Peripherals, USB};

const CPU_FREQ: usize = {
    // 8 MHz HSE
    // PLL multiplier of 6
    // 8x divider for cortex system timer input
    // the SYSCLK should be 48MHz since the PLL should be the SYSCLK source
    // and the PLL is 48Mhz to run the USB
    // i had always assumed SYSCLK was the cpu speed, but using 48E6 as the delay, it takes ~8 s
    // the cortex system timer matches the cycle speed, it has a prescaler of 8, so it lines up
    // need to meet constraints:
    //  USB input clock == 48 MHz
    //  HSE is required to be input clock to USB, per that other datasheet that's 149 pages
    //  APB1 clock [10, 36] MHz
    //      36 is the upper bound for APB1 in all cases
    //      10 MHz is a lower bound set by operation of the USB, it otherwise is not required

    // NOTE:
    // the datasheet says when the HSE is switched on, the relevant pins are taken over
    // and that no GPIO configuration bits have effect
    // the pins are PF0/1 for the HSE, and there is PC14/15 for OSC32
    // PF0 and PC14 are IN, PF1 and PC15 are OUT

    // with the provided hardware:
    // HSE 8MHz can be PLL input clock
    // PLL can do a multiplier of 6
    // this makes a 48 MHz PLL clock, which feeds USB directly

    // we can set PLL as SYSCLK
    // an AHB prescaler of 4
    // this leaves an HCLK and APB1 clock of 12 MHz

    // as a consequence, the SysTick is running at SYSLCK / 8 == 6 MHz
    8E6 as usize * 6 / 8
};

const fn rx_buffer_size_bytes(bf: u16) -> usize {
    let bl_size = 0b1 & (bf >> 15);
    let num_block = 0x1f & (bf >> 10);

    if 0 == bl_size && 0 == num_block {
        panic!("not possible");
    }

    let bytes = if bl_size == 0 {
        num_block * 2
    } else {
        (num_block + 1) * 32
    };

    if bytes >= 512 {
        panic!("this is not supported in my datasheet");
    }

    bytes as usize
}

// EP0 is always enabled, is always a control endpoint
// EP1 = host IN, a TX only bulk endpoint
// EP2 = host OUT, an RX only bulk endpoint
const USB_NUM_ACTIVE_ENDPOINTS: usize = 3;

const USB_SRAM_BEGIN: u32 = 0x4000_6000;
// we are reusing the buffer on EP0 for both TX and RX
const USB_EP0_RX_BUFFER_SIZE_BITFIELD: u16 = 0b0_00100_0000000000;
const USB_EP0_RX_BUFFER_SIZE_BYTES: usize = rx_buffer_size_bytes(USB_EP0_RX_BUFFER_SIZE_BITFIELD);
const USB_EP0_TX_BUFFER_SIZE_BYTES: usize = USB_EP0_RX_BUFFER_SIZE_BYTES;
const USB_EP0_RX_BUFFER_SIZE_WORDS: usize = USB_EP0_RX_BUFFER_SIZE_BYTES / 2;
const USB_EP0_TX_BUFFER_SIZE_WORDS: usize = USB_EP0_RX_BUFFER_SIZE_WORDS;

// each endpoint in the descriptor table needs 4 words/8 bytes
// we have 3 endpoints
const USB_EP0_TX_BUFFER_BEGIN: usize = 4 * USB_NUM_ACTIVE_ENDPOINTS;
const USB_EP0_RX_BUFFER_BEGIN: usize = USB_EP0_TX_BUFFER_BEGIN;
//const USB_EP0_RX_BUFFER_BEGIN: usize = USB_EP0_TX_BUFFER_BEGIN + (USB_EP0_TX_BUFFER_SIZE_BYTES / 2);
// HEY: the enumeration process seems to be synchronous, so we can re-use the buffers for enumeration

// BL_SIZE = 1, NUM_BLOCK = 0, buffer should be 32 bytes per the datasheet's lookup table
const USB_EP2_RX_BUFFER_SIZE_BYTES_BITFIELD: u16 = 0b1_00000_0000000000;
const USB_EP2_RX_BUFFER_SIZE_BYTES: usize =
    rx_buffer_size_bytes(USB_EP2_RX_BUFFER_SIZE_BYTES_BITFIELD);
const USB_EP1_TX_BUFFER_SIZE_BYTES: usize = USB_EP2_RX_BUFFER_SIZE_BYTES;
const USB_EP1_TX_BUFFER_BEGIN: usize = USB_EP0_RX_BUFFER_BEGIN + USB_EP0_TX_BUFFER_SIZE_WORDS;
const USB_EP2_RX_BUFFER_BEGIN: usize = USB_EP1_TX_BUFFER_BEGIN + (USB_EP1_TX_BUFFER_SIZE_BYTES / 2);

#[repr(C)]
#[allow(non_camel_case_types)]
union Endpoint_BTABLE_Addr_Register {
    _reserved: u32,
    v: u16,
}

#[repr(C)]
#[allow(non_camel_case_types)]
union Endpoint_BTABLE_Count_Register {
    _reserved: u32,
    v: u16,
}

#[repr(C)]
#[allow(non_camel_case_types)]
struct Endpoint_BTABLE_Entry {
    addr_tx: Endpoint_BTABLE_Addr_Register,
    count_tx: Endpoint_BTABLE_Count_Register,
    addr_rx: Endpoint_BTABLE_Addr_Register,
    count_rx: Endpoint_BTABLE_Count_Register,
}

#[derive(Debug)]
#[allow(non_camel_case_types)]
struct EP0_Tx {
    w_length: u16,
    tx_need: u16,
    tx_acc: u16,
}

impl Endpoint_BTABLE_Addr_Register {
    fn set(&mut self, v: u16) {
        self.v = v * 2;
    }

    fn get(&self) -> u16 {
        (unsafe { self.v }) / 2
    }
}

impl Endpoint_BTABLE_Count_Register {
    fn set(&mut self, v: u16) {
        self.v = v;
    }

    fn get(&self) -> u16 {
        unsafe { self.v }
    }
}

impl Endpoint_BTABLE_Entry {
    fn at(ep: usize) -> *mut Endpoint_BTABLE_Entry {
        if ep > 7 {
            panic!("invalid endpoint {}", ep)
        }

        (USB_SRAM_BEGIN as *mut Endpoint_BTABLE_Entry).wrapping_byte_add(ep * 16)
    }
}

#[derive(Clone, Copy)]
#[repr(C)]
#[allow(non_camel_case_types)]
union SRAM_Word {
    _reserved: u32,
    s: i16,
    u: u16,
}

fn dump_sram<'a>(in_buf: &'a [SRAM_Word], out_buf: &'a mut [u16]) {
    out_buf
        .iter_mut()
        .enumerate()
        .for_each(|(i, w)| *w = unsafe { in_buf[i].u });
}

const DEVICE_DESCRIPTOR: &[SRAM_Word] = &[
    SRAM_Word { u: 0x1201 }, // bLength, bDescriptorType
    SRAM_Word { u: 0x0200 }, // bcdUSB
    SRAM_Word { u: 0x0200 }, // bDeviceClass, bDeviceSubClass
    SRAM_Word {
        u: USB_EP0_RX_BUFFER_SIZE_BYTES as u16,
    }, // bDeviceProtocol (not shown = 0), bMaxPacketSize
    SRAM_Word {
        u: 0xDEAD_u16.swap_bytes(),
    }, // idVendor
    SRAM_Word {
        u: 0xBEEF_u16.swap_bytes(),
    }, // idProduct
    SRAM_Word {
        u: 0xCAFE_u16.swap_bytes(),
    }, // bcdDevice
    SRAM_Word { u: 0x0000 }, // iManufacturer, iProduct
    SRAM_Word { u: 0x0001 }, // iSerialNumber, bNumConfigurations
];

// TODO: some of these transmit in the opposite order for some reason
// TODO: the endpoint sizes are controlled by a constant, we should use those
const CONFIG_DESCRIPTOR: &[SRAM_Word] = &[
    SRAM_Word { u: 0x0902 }, // bLength, bDescriptorType
    SRAM_Word {
        u: 0x0020_u16.swap_bytes(),
    }, // wTotalLength
    SRAM_Word { u: 0x0101 }, // bNumInterfaces, bConfigurationValue
    SRAM_Word { u: 0x0080 }, // iConfiguration, bmAttributes
    SRAM_Word { u: 0x6409 }, // bMaxPower, interface descriptor bLength
    SRAM_Word { u: 0x0400 }, // bDescriptorType, bInterfaceNumber
    SRAM_Word { u: 0x0002 }, // bAlternateSetting, bNumEndpoints
    SRAM_Word { u: 0x0A00 }, // bInterfaceClass, bInterfaceSubClass
    SRAM_Word { u: 0x0000 }, // bInterfaceProtocol, iInterface
    SRAM_Word { u: 0x0705 }, // EP1 bLength, bDescriptorType
    SRAM_Word { u: 0x8102 }, // bEndpointAddress, bmAttributes
    SRAM_Word {
        //u: USB_EP1_TX_BUFFER_SIZE_BYTES as u16
        u: 0x0020_u16.swap_bytes(),
    }, // wMaxPacketSize
    SRAM_Word { u: 0x0407 }, // bInterval, EP2 bLength
    SRAM_Word { u: 0x0502 }, // bDescriptorType, bEndpoitnAddress
    SRAM_Word { u: 0x0220 }, // bmAttributes, wMaxPacketSize low byte
    //u: (USB_EP2_RX_BUFFER_SIZE_BYTES as u16).swap_bytes()[0] or 1?
    SRAM_Word { u: 0x0004 }, // wMaxPacketSize high byte, bInterval
                             //u: (USB_EP2_RX_BUFFER_SIZE_BYTES as u16).swap_bytes()[1] or 0?
];

#[derive(Debug)]
#[allow(non_camel_case_types)]
enum USB_Send {
    Descriptor {
        bm_desc_type: u8,
        bm_desc_idx: u8,
        state: EP0_Tx,
    },
    Status {
        n_bytes: u8,
    },
}

#[derive(Debug)]
#[allow(non_camel_case_types)]
enum USB_Setup {
    U_Await_Reset,
    U_Await_Control,
    U_Send {
        bm_desc_type: u8,
        bm_desc_idx: u8,
        state: EP0_Tx,
    },
    U_Status,
    Addressing {
        new_addr: u8,
    },
    A_Await_Control,
    A_Send {
        bm_desc_type: u8,
        bm_desc_idx: u8,
        state: EP0_Tx,
    },
    A_Status,
}

// TODO: once we enumerate and have I/O to arbitrary endpoints, merge these
// because they should be logically tied in terms of critical section
// holding the USB should mean you also control the enumeration state, even if it doesn't change
static USB: Mutex<RefCell<Option<USB>>> = Mutex::new(RefCell::new(None));
static USB_ENUMERATION: Mutex<RefCell<USB_Setup>> =
    Mutex::new(RefCell::new(USB_Setup::U_Await_Reset));
static USB_TX_READY: Mutex<RefCell<bool>> = Mutex::new(RefCell::new(false));

fn sram_buffer(word_offset: usize, num_words: usize) -> &'static mut [SRAM_Word] {
    let sram_base = USB_SRAM_BEGIN as *mut SRAM_Word;
    let addr = sram_base.wrapping_add(word_offset);

    #[cfg(debug_assertions)]
    {
        const USB_SRAM_END: u32 = 0x4000_6200;
        let sram_end = USB_SRAM_END as *mut SRAM_Word;
        let addr_end = addr.wrapping_byte_add(num_words * 2);

        //rprintln!("{:?} {:?} {:?} {:?} {} {}", sram_base, sram_end, addr, addr_end, word_offset, num_words);

        assert!(addr >= sram_base, "not {addr:?} >= {sram_base:?}");
        assert!(addr <= sram_end, "not {addr:?} <= {sram_end:?}");
        assert!(addr_end <= sram_end, "not {addr_end:?} <= {sram_end:?}");
    }

    unsafe { core::slice::from_raw_parts_mut(addr, num_words) }
}

fn ep_tx_rx_buffers(
    ep_bd: *mut Endpoint_BTABLE_Entry,
    tx_byte_size: usize,
    rx_byte_size: usize,
) -> (&'static mut [SRAM_Word], &'static mut [SRAM_Word]) {
    // addr field accessors return word offsets
    // count fields contain number of bytes
    let (at, ar) = unsafe { ((*ep_bd).addr_tx.get(), (*ep_bd).addr_rx.get()) };

    (
        sram_buffer(at as usize, tx_byte_size / 2),
        sram_buffer(ar as usize, rx_byte_size / 2),
    )
}

fn split_sram_word(word: &SRAM_Word) -> (u8, u8) {
    let b = unsafe { word.u.to_le_bytes() };

    // the datasheet says that the first byte received is stored as the least significant byte
    // the USB standard and this chip both store/process things as little endian
    // if a field in a USB packet is 2 bytes 0xABCD, then the debug output should be 0xABCD and
    // internally if you view raw memory without interpretation, it should show as 0xCDAB
    // but if a field in a USB packet is 1 byte 0xAB, and the other byte is not of interest 0xEF
    // then i should see it in a debug statement as 0xEFAB
    // so the fact i'm calling this function means i need to reverse those bytes
    //
    // i could use the to_be_bytes() function to make 0xABCD look like 0xAB 0xCD since '0xABCD' is
    // already represnted in big endian, then i'd return indices (1, 0)
    // so this is just me leaving a note that i'm not reversing this intentionally
    (b[0], b[1])
}

fn write_descriptor(
    descriptor_type: u8,
    descriptor_index: u8,
    ep0_bd: *mut Endpoint_BTABLE_Entry,
    w_length: u16,
    accumulated: u16,
) -> (u16, u16) {
    let (buf, _) = ep_tx_rx_buffers(
        ep0_bd,
        USB_EP0_TX_BUFFER_SIZE_BYTES,
        USB_EP0_RX_BUFFER_SIZE_BYTES,
    );

    let desc_ref = match (descriptor_index, descriptor_type) {
        (_, 1) => DEVICE_DESCRIPTOR,
        (_, 2) => CONFIG_DESCRIPTOR,
        (_, 4) => todo!("implement interface descriptor from config descriptor"),
        (_, 5) => todo!("implement endpoint descriptor from config descriptor"),
        (_, 3) => todo!("implement string descriptors"),
        _ => unreachable!("write descriptor: {} {}", descriptor_type, descriptor_index),
    };

    let desc_size = (desc_ref.len() * 2) as u16;
    let tx_need = core::cmp::min(w_length, desc_size);
    let remaining = tx_need - accumulated;
    let word_offset = (accumulated as usize) / 2;

    let tx_size = core::cmp::min(remaining, USB_EP0_TX_BUFFER_SIZE_BYTES as u16);

    for (i, w) in desc_ref
        .iter()
        // skip any already transmitted bytes
        .skip(word_offset)
        // only fill up to as much as the buffer will hold
        .take(buf.len())
        .enumerate()
    {
        let u = unsafe { w.u.swap_bytes() };
        buf[i] = SRAM_Word { u }
    }

    (tx_need, tx_size)
}

fn to_valid(stat_bits: u8) -> u8 {
    // TODO: does the PAC handle this correctly?
    // if i did epnr.stat_tx().valid(), would it produce the correct toggle bits, or does it
    // produce the constant sequence 0b11?

    match stat_bits {
        0b00 => 0b11,
        0b01 => 0b10,
        0b10 => 0b01,
        0b11 => 0b00,
        _ => unreachable!(),
    }
}

fn to_nak(stat_bits: u8) -> u8 {
    // TODO: see the above TODO about this pac's handling of toggle bits
    match stat_bits {
        0b00 => 0b10,
        0b01 => 0b11,
        0b10 => 0b00,
        0b11 => 0b01,
        _ => unreachable!(),
    }
}

//fn to_stall(stat_bits: u8) -> u8 {
//    //TODO: see the above TODO about this pac's handling of toggle bits
//
//    match stat_bits {
//        0b00 => 0b01,
//        0b01 => 0b00,
//        0b10 => 0b11,
//        0b11 => 0b10,
//        _ => unreachable!(),
//    }
//}

fn to_disabled(stat_bits: u8) -> u8 {
    //TODO: see the above TODO about this pac's handling of toggle bits
    // this one is easy, we just need to get to 0
    // so whatever the xor value for x is to get 0
    // which x ^ x == 0
    stat_bits
}

fn handle_ep0(usb: &USB, handle_tx: bool, handle_rx: bool, stat_tx: u8, stat_rx: u8) -> (u8, u8) {
    let mut toggle_tx = to_nak(stat_tx);
    let ep_bd = Endpoint_BTABLE_Entry::at(0);

    assert!(
        !(handle_tx && handle_rx),
        "ep0 should be entirely synchronous, so how do we do both? we are sharing the TX and RX buffers too"
    );

    if handle_tx {
        let mut count_tx = 0;

        critical_section::with(|cs| {
            USB_ENUMERATION
                .borrow(cs)
                .replace_with(|this_stage| match this_stage {
                    USB_Setup::U_Send {
                        bm_desc_type: t,
                        bm_desc_idx: i,
                        state: s,
                    } => {
                        assert!(0 == usb.daddr().read().add().bits());

                        if s.tx_acc >= s.tx_need {
                            USB_Setup::U_Status
                        } else {
                            toggle_tx = to_valid(stat_tx);

                            // U_Send more data, remain in the sending state
                            let w_length = s.w_length;
                            let tx_acc = s.tx_acc;
                            let tx_need = s.tx_need;
                            let (_, new_count_tx) =
                                write_descriptor(*t, *i, ep_bd, w_length, tx_acc);

                            count_tx = new_count_tx;

                            let new_state = EP0_Tx {
                                w_length,
                                tx_need,
                                tx_acc: tx_acc + count_tx,
                            };

                            USB_Setup::U_Send {
                                bm_desc_type: *t,
                                bm_desc_idx: *i,
                                state: new_state,
                            }
                        }
                    }
                    USB_Setup::U_Status => USB_Setup::U_Await_Control,
                    USB_Setup::Addressing { new_addr: t } => {
                        usb.daddr().modify(|_, w| unsafe { w.add().bits(*t) });

                        USB_Setup::A_Await_Control
                    }
                    USB_Setup::A_Send {
                        bm_desc_type: t,
                        bm_desc_idx: i,
                        state: s,
                    } => {
                        assert!(0 != usb.daddr().read().add().bits());

                        if s.tx_acc >= s.tx_need {
                            USB_Setup::A_Status
                        } else {
                            toggle_tx = to_valid(stat_tx);

                            // U_Send more data, remain in the sending state
                            let w_length = s.w_length;
                            let tx_need = s.tx_need;
                            let tx_acc = s.tx_acc;
                            let (_, new_count_tx) =
                                write_descriptor(*t, *i, ep_bd, w_length, tx_acc);

                            count_tx = new_count_tx;

                            let new_state = EP0_Tx {
                                w_length,
                                tx_need,
                                tx_acc: tx_acc + count_tx,
                            };

                            USB_Setup::A_Send {
                                bm_desc_type: *t,
                                bm_desc_idx: *i,
                                state: new_state,
                            }
                        }
                    }
                    USB_Setup::A_Status => USB_Setup::A_Await_Control,
                    USB_Setup::A_Await_Control => USB_Setup::A_Await_Control,
                    _ => unreachable!(
                        "tx complete interrupt, but state was wrong: {:?}",
                        this_stage
                    ),
                })
        });

        unsafe { (*ep_bd).count_tx.set(count_tx) };
    }

    if handle_rx {
        // handle the RX after the TX, since the TX may say "we are done"
        // but now the RX may feed more data in, thereby setting it as "we have data to U_Send"
        let rx_count = unsafe { (*ep_bd).count_rx.get() & 0x01ff };

        if 0 < rx_count {
            let (_, rx_content) =
                ep_tx_rx_buffers(ep_bd, USB_EP0_RX_BUFFER_SIZE_BYTES, rx_count as usize);
            let word = rx_content[0];

            let (bm_request_type, bm_request) = split_sram_word(&word);

            let w_value = unsafe { rx_content[1].u };
            let _w_index = unsafe { rx_content[2].u };
            let w_length = unsafe { rx_content[3].u };

            let state_change = match (bm_request_type, bm_request) {
                (0x80, 0x00) => unimplemented!("device get status"),
                (0x00, 0x01) => unimplemented!("device clear feature"),
                (0x00, 0x03) => unimplemented!("device set feature"),
                (0x00, 0x05) => {
                    // device set address

                    unsafe { (*ep_bd).count_tx.set(0) };

                    toggle_tx = to_valid(stat_tx);

                    USB_Setup::Addressing {
                        new_addr: w_value as u8,
                    }
                }
                (0x80, 0x06) => {
                    // device get descriptor

                    toggle_tx = to_valid(stat_tx);

                    //let tx_capacity = if 0 == w_length & 0b1 {
                    //    core::cmp::min(USB_EP0_TX_BUFFER_SIZE_BYTES as u16, w_length & 0xFFFE)
                    //} else {
                    //    core::cmp::min(USB_EP0_TX_BUFFER_SIZE_BYTES as u16, w_length + 1)
                    //};

                    let (bm_desc_idx, bm_desc_type) = split_sram_word(&SRAM_Word { u: w_value });

                    let (tx_need, tx_acc) =
                        write_descriptor(bm_desc_type, bm_desc_idx, ep_bd, w_length, 0);

                    unsafe { (*ep_bd).count_tx.set(tx_acc) };

                    let state = EP0_Tx {
                        w_length,
                        tx_need,
                        tx_acc,
                    };

                    if 0 == usb.daddr().read().add().bits() {
                        USB_Setup::U_Send {
                            bm_desc_type,
                            bm_desc_idx,
                            state,
                        }
                    } else {
                        USB_Setup::A_Send {
                            bm_desc_type,
                            bm_desc_idx,
                            state,
                        }
                    }
                }
                (0x00, 0x07) => unimplemented!("device set descriptor {} {}", w_value, w_length),
                (0x80, 0x08) => unimplemented!("device get configuration {} {}", w_value, w_length),
                (0x00, 0x09) => {
                    toggle_tx = to_valid(stat_tx);

                    {
                        let endpoint = 1;
                        let ep_bd = Endpoint_BTABLE_Entry::at(endpoint);
                        let epr = usb.epr(endpoint);

                        unsafe {
                            (*ep_bd).addr_tx.set(USB_EP1_TX_BUFFER_BEGIN as u16);
                            (*ep_bd).count_tx.set(0); // no data to U_Send
                        }

                        unsafe {
                            epr.write(|w| {
                                w.ep_type()
                                    .bulk()
                                    .stat_tx()
                                    .valid()
                                    .ea()
                                    .bits(endpoint as u8)
                            });
                        }

                        critical_section::with(|cs| {
                            USB_TX_READY.borrow(cs).replace(true);
                        });
                    }

                    {
                        let endpoint = 2;
                        let ep_bd = Endpoint_BTABLE_Entry::at(endpoint);
                        let epr = usb.epr(endpoint);

                        unsafe {
                            (*ep_bd).addr_rx.set(USB_EP2_RX_BUFFER_BEGIN as u16);
                            (*ep_bd).count_rx.set(USB_EP2_RX_BUFFER_SIZE_BYTES_BITFIELD);
                        }

                        unsafe {
                            epr.write(|w| {
                                w.ep_type()
                                    .bulk()
                                    .stat_rx()
                                    .valid()
                                    .ea()
                                    .bits(endpoint as u8)
                            });
                        }
                    }

                    USB_Setup::A_Await_Control

                    //unimplemented!("device set configuration {} {}", w_value, w_length)
                }

                (0x81, 0x00) => unimplemented!("interface get status"),
                (0x01, 0x01) => unimplemented!("interface clear feature"),
                (0x01, 0x03) => unimplemented!("interface set feature"),
                (0x81, 0x0A) => unimplemented!("interface get alternate interface"),
                (0x01, 0x11) => unimplemented!("interface set alternate interface"),

                (0x82, 0x00) => unimplemented!("endpoint get status"),
                (0x02, 0x01) => unimplemented!("endpoint clear feature"),
                (0x02, 0x03) => unimplemented!("endpoint set feature"),
                (0x82, 0x12) => unimplemented!("endpoint sync frame"),

                _ => {
                    unimplemented!("({:#x},{:#x}) cant decode", bm_request_type, bm_request);
                }
            };

            critical_section::with(|cs| {
                USB_ENUMERATION.borrow(cs).replace(state_change);
            });
        } else if handle_rx {
            rprintln!("rx: 0");
        }
    }

    (toggle_tx, to_valid(stat_rx))
}

fn handle_ep1(usb: &USB, handle_tx: bool, _handle_rx: bool, stat_tx: u8, _stat_rx: u8) -> (u8, u8) {
    assert!(
        handle_tx,
        "ep1 is IN, therefore it must be able to handle TX"
    );

    let endpoint = 1;
    let epr = usb.epr(endpoint);
    let ep_bd = Endpoint_BTABLE_Entry::at(endpoint);

    epr.modify(|_, w| w.ctr_tx().clear_bit());

    rprintln!("ep1 tx {}", unsafe { (*ep_bd).count_tx.get() & 0x007F });

    unsafe {
        (*ep_bd).count_tx.set(0);
    }

    // TX finished, re-ready
    critical_section::with(|cs| USB_TX_READY.borrow(cs).replace(true));

    (to_nak(stat_tx), 0)
}

fn handle_ep2(usb: &USB, _handle_tx: bool, handle_rx: bool, _stat_tx: u8, stat_rx: u8) -> (u8, u8) {
    assert!(
        handle_rx,
        "ep2 is OUT, therefore it must be able to handle RX"
    );

    let endpoint = 2;
    let epr = usb.epr(endpoint);

    epr.modify(|_, w| w.ctr_rx().clear_bit());

    let ep_bd = Endpoint_BTABLE_Entry::at(endpoint);

    let (_, rx_buf) = ep_tx_rx_buffers(ep_bd, 0, USB_EP2_RX_BUFFER_SIZE_BYTES);

    let mut out_buf = [0_u16; const { USB_EP2_RX_BUFFER_SIZE_BYTES / 2 }];
    dump_sram(rx_buf, &mut out_buf);
    rprintln!(
        "ep2 rx {} = {:?}",
        unsafe { (*ep_bd).count_rx.get() & 0x007F },
        out_buf
    );

    unsafe { (*ep_bd).count_rx.set(USB_EP2_RX_BUFFER_SIZE_BYTES_BITFIELD); }

    (0, to_valid(stat_rx))
}

fn usb_handler() {
    let usb = critical_section::with(|cs| USB.borrow(cs).borrow_mut().take().unwrap());
    let istr = usb.istr().read();
    let endpoint = istr.ep_id().bits() as usize;

    let epnr = usb.epr(endpoint);
    let epnr_check = usb.epr(endpoint).read();

    #[cfg(debug_assertions)]
    {
        assert!(0 == usb.btable().read().bits(), "btable was non zero");
        assert!(!istr.pmaovr().bit(), "pmaovr was set");
        assert!(!istr.err().bit(), "err was set");
        assert!(0 == endpoint, "endpoint was not 0");

        // datasheet says FNR is safe to read when there is an SOF interrupt
        //if istr.sof().bit() {
        //    // these should only flip when ESOF interrupts are generated
        //    let lsof = usb.fnr().read().lsof().bits();

        //    assert!(0 == lsof, "lsof was non-zero");
        //}
    }

    usb.istr().modify(|_, w| {
        w.wkup()
            .clear_bit()
            .susp()
            .clear_bit()
            .esof()
            .clear_bit()
            .sof()
            .clear_bit()
    });

    // reload this
    let istr = usb.istr();
    let istr_check = istr.read();
    let stat_tx = epnr_check.stat_tx().bits();
    let stat_rx = epnr_check.stat_rx().bits();

    if !(istr_check.reset().bit() || istr_check.ctr().bit() || epnr_check.setup().bit()) {
        // if this isn't a meaningful packet then skip it

        #[cfg(debug_assertions)]
        {
            rprintln!("what was this packet for? {}", istr_check.bits());
        }

        critical_section::with(|cs| {
            USB.borrow(cs).replace(Some(usb));
        });

        return;
    } else if istr_check.reset().bit() {
        #[cfg(debug_assertions)]
        {
            assert!(0 == endpoint);
        }

        usb.istr().modify(|_, w| w.reset().clear_bit());
        usb.btable().reset();

        // enable, zero out the address
        usb.daddr()
            .write(|w| unsafe { w.ef().enabled().add().bits(0) });

        let ep0_btable = Endpoint_BTABLE_Entry::at(0);

        // implement the default control endpoint
        unsafe {
            (*ep0_btable).addr_tx.set(USB_EP0_TX_BUFFER_BEGIN as u16);
            (*ep0_btable).addr_rx.set(USB_EP0_RX_BUFFER_BEGIN as u16);

            // stores the number of bytes in TX buffer to U_Send, not the size of teh buffer
            (*ep0_btable).count_tx.set(0);

            // encodes the size of the buffer, and is later additionally written the size of the RX
            // data
            (*ep0_btable).count_rx.set(USB_EP0_RX_BUFFER_SIZE_BITFIELD);

            epnr.write(|w| {
                w.ctr_rx()
                    .clear_bit()
                    .dtog_rx()
                    .bit(epnr_check.dtog_rx().bit()) // if bit was set, this should clear it
                    .stat_rx()
                    .bits(to_valid(stat_rx))
                    // cannot modify setup bit
                    .ep_type()
                    .control()
                    .ep_kind()
                    .bit(false)
                    .ctr_tx()
                    .clear_bit()
                    .dtog_tx()
                    .bit(epnr_check.dtog_tx().bit()) // if bit was set, this should clear it
                    .stat_tx()
                    .bits(to_disabled(stat_tx))
                    .ea()
                    .bits(0)
            });
        }

        critical_section::with(|cs| {
            USB_ENUMERATION
                .borrow(cs)
                .replace(USB_Setup::U_Await_Control);
        });

        critical_section::with(|cs| {
            USB.borrow(cs).replace(Some(usb));
        });

        return;
    }

    let handle_tx = epnr_check.ctr_tx().bit();
    // only ep0 should be getting the setup bit
    let handle_rx = epnr_check.ctr_rx().bit() || epnr_check.setup().bit();

    // the datasheet says we need to clear the TX/RX bits before processing
    // this should ONLY clear those bits, and set everything else to an invariant value
    epnr.modify(|_, w| w.ctr_rx().clear_bit().ctr_tx().clear_bit());

    critical_section::with(|cs| {
        let stage = USB_ENUMERATION.borrow(cs).borrow();

        rprintln!("{:?}", stage);
    });

    let (toggle_tx, toggle_rx) = match endpoint {
        0 => handle_ep0(&usb, handle_tx, handle_rx, stat_tx, stat_rx),
        1 => handle_ep1(&usb, handle_tx, handle_rx, stat_tx, stat_rx),
        2 => handle_ep2(&usb, handle_tx, handle_rx, stat_tx, stat_rx),
        _ => todo!("implement endpoint {}", endpoint),
    };

    epnr.modify(|_, w| unsafe { w.stat_rx().bits(toggle_rx).stat_tx().bits(toggle_tx) });

    critical_section::with(|cs| {
        USB.borrow(cs).replace(Some(usb));
    });
}

#[interrupt]
fn USB_HP() {
    rprintln!("hp");

    usb_handler();
}

#[interrupt]
fn USB_WKUP() {
    rprintln!("wkup");

    usb_handler();
}

#[interrupt]
fn USB_HP_CAN_TX() {
    rprintln!("can_tx");

    usb_handler();
}

#[interrupt]
fn USB_LP_CAN_RX0() {
    //rprintln!("rx0");

    usb_handler();
}

#[interrupt]
fn USB_LP() {
    rprintln!("lp");

    usb_handler();
}

#[interrupt]
fn USB_WKUP_EXTI() {
    rprintln!("exti");

    usb_handler();
}

#[cortex_m_rt::entry]
fn main() -> ! {
    rtt_init_print!();

    let peripherals = Peripherals::take().unwrap();
    let rcc = &peripherals.RCC;
    let gpioa = &peripherals.GPIOA;
    let usb = peripherals.USB;

    rprintln!("begin");

    unsafe {
        // i read somewhere that someone's USB sucked because it was too early to start up
        asm::delay(CPU_FREQ as u32 / 1000 * 100);

        // turn off PLL, turn on HSE
        rcc.cr().write(|w| w.pllon().clear_bit().hseon().on());

        // wait for PLL to be not ready anymore
        while rcc.cr().read().pllrdy().bit() {
            asm::nop();
        }

        // wait for the HSE to be ready
        while !rcc.cr().read().hserdy().bit() {
            asm::nop();
        }

        rcc.cfgr().write(|w| {
            w.usbpre()
                .div1() // USB input clock is not divided by 1.5
                .pllmul()
                .mul6() // PLL has a 6x multipler
                .pllxtpre()
                .div1() // HSE input to PLL not divided
                .pllsrc()
                .hse_div_prediv() // PLL input clock is HSE
                .hpre()
                .div4() // HCLK/AHB == SYSCLK/4 == 12 MHz
        });

        // turn on PLL
        rcc.cr().modify(|_, w| w.pllon().on());

        // wait for PLL to be ready
        while !rcc.cr().read().pllrdy().bit() {
            asm::nop();
        }

        // now that PLL is ready, swap to use it as sysclk source
        rcc.cfgr().modify(|_, w| w.sw().bits(0b10));

        // enable GPIOA peripheral clock
        rcc.ahbenr().write(|w| w.iopaen().enabled());

        //#[cfg(debug_assertions)]
        {
            //rprintln!("doing a D+ reset");
            // F3 Discovery board has a pull-up resistor on the D+ line.
            // Pull the D+ pin down to U_Send a RESET condition to the USB bus.
            // This forced reset is needed only for development, without it host
            // will not reset your device when you upload new firmware.
            gpioa
                .moder()
                .write(|w| w.moder12().output().moder11().output()); // general purpose output
            gpioa
                .otyper()
                .write(|w| w.ot12().push_pull().ot11().push_pull()); // output push pull
            gpioa.odr().write(|w| w.odr12().low()); // pull low

            // 4 millisecond
            asm::delay(CPU_FREQ as u32 / 1000 * 4);

            // pin 12 is now free to be used for USB
        }

        gpioa
            .moder()
            .write(|w| w.moder11().alternate().moder12().alternate());

        // set pa11/12 as push-pull outputs
        //gpioa
        //    .otyper()
        //    .write(|w| w.ot11().push_pull().ot12().push_pull());

        // set alternate functions for PA9/PA10 to USB, which is AF 14
        gpioa.afrh().write(|w| w.afrh11().af14().afrh12().af14());

        // disable GPIOA peripheral clock for power savings i guess
        rcc.ahbenr().modify(|_, w| w.iopaen().disabled());

        // summary, per the datasheet:
        //  provide clock
        //  de-assert macrocell reset signal (i think this is APB1RSTR.USBRST bit in RCC)
        //  after that:
        //      clear PDWN bit
        //      wait t_STARTUP amount of time (1 microsecond per the other datasheet)
        //      remove FRES bit
        //      clear ISTR register

        // enable USB clock
        rcc.apb1enr().write(|w| w.usben().enabled());

        // clear USB sram, 256 u16
        let usb_sram = sram_buffer(0, 256);
        //let buf = Endpoint_BTABLE_Entry::at(0);

        //rprintln!("sram before clear: {:?}", usb_sram);
        //rprintln!("buf before clear: {}", *buf);
        usb_sram.fill(SRAM_Word { u: 0 });
        //rprintln!("sram after clear: {:?}", usb_sram);
        //rprintln!("buf after clear: {}", *buf);

        // reset USB
        rcc.apb1rstr().write(|w| w.usbrst().reset());

        // un-reset USB
        rcc.apb1rstr().write(|w| w.usbrst().clear_bit());

        // power down and reset
        usb.cntr().write(|w| w.pdwn().enabled().fres().reset());

        // clear powerdown
        usb.cntr().write(|w| w.pdwn().disabled().fres().reset());

        // t_STARTUP is 1 microsecond, and must be waited after clearing pdwn
        asm::delay(CPU_FREQ as u32 / 1_000_000 * 2);

        usb.cntr().write(|w| w.pdwn().disabled().fres().no_reset());

        // clear any pending interrupts
        usb.istr().reset();

        // enable interrupts
        usb.cntr().modify(|_, w| {
            w.ctrm()
                .enabled()
                .pmaovrm()
                .set_bit()
                .errm()
                .set_bit()
                .wkupm()
                .clear_bit()
                .suspm()
                .clear_bit()
                .resetm()
                .enabled()
                .sofm()
                .clear_bit()
                .esofm()
                .clear_bit()
        });

        critical_section::with(|cs| {
            USB.borrow(cs).replace(Some(usb));
        });

        // these access the USB struct, hence we needed to store it before enabling interrupts
        NVIC::unmask(interrupt::USB_HP);
        NVIC::unmask(interrupt::USB_WKUP);
        NVIC::unmask(interrupt::USB_HP_CAN_TX);
        NVIC::unmask(interrupt::USB_LP_CAN_RX0);
        NVIC::unmask(interrupt::USB_LP);
        NVIC::unmask(interrupt::USB_WKUP_EXTI);

        loop {
            asm::wfi();

            critical_section::with(|cs| {
                let mut b = USB_TX_READY.borrow(cs).borrow_mut();

                if *b {
                    let ep_bd = Endpoint_BTABLE_Entry::at(1);
                    let (tx_buf, _) = ep_tx_rx_buffers(ep_bd, USB_EP1_TX_BUFFER_SIZE_BYTES, 0);

                    for (i, w) in tx_buf.iter_mut().enumerate().take(10) {
                        (*w).u = (0x30 + i) as u16;
                    }

                    let usb = USB.borrow(cs).take().unwrap();

                    (*ep_bd).count_tx.set(10);

                    usb.epr(1)
                        .modify(|r, w| w.stat_tx().bits(to_valid(r.stat_tx().bits())));

                    USB.borrow(cs).replace(Some(usb));

                    *b = false;
                }
            });
        }
    }
}

