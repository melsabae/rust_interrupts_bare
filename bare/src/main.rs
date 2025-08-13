#![no_std]
#![no_main]

// pick a panicking behavior
// use panic_halt as _; // you can put a breakpoint on `rust_begin_unwind` to catch panics
// use panic_abort as _; // requires nightly
// use panic_itm as _; // logs messages over ITM; requires ITM support
// use panic_semihosting as _; // logs messages to the host stderr; requires a debugger

use core::arch::asm;
use core::ptr;
//use core::sync::atomic::{AtomicUsize, Ordering};
use cortex_m_rt::entry;

//static COUNTER: AtomicUsize = AtomicUsize::new(0);
const USART1_BASE: *mut u32 = 0x4001_3800 as *mut u32;

unsafe extern "C" {
    fn DefaultHandler();
    fn EXTI0_Handler();
}

#[unsafe(no_mangle)]
fn exti0_handler() {
    let exti_base = 0x4001_0400 as *mut u32;
    let exti_pr1 = exti_base.wrapping_byte_add(0x14);
    let exti0_pr1_clear = 0b1_u32; // exti0 source interrupt
    let usart1_tdr = USART1_BASE.wrapping_byte_add(0x28) as *mut u8;

    unsafe {
        // clear exti0 interrupt pending
        ptr::write_volatile(exti_pr1, exti0_pr1_clear);

        // send a byte over usart
        ptr::write_volatile(usart1_tdr, b'$'); // '$'
    }
}

#[entry]
fn main() -> ! {
    // set pointers
    let rcc_base = 0x4002_1000 as *mut u32;
    let gpio_a_base = 0x4800_0000 as *mut u32;
    let syscfg_base = 0x4001_0000 as *mut u32;
    let exti_base = 0x4001_0400 as *mut u32;
    //let nvic_base = 0xe000_e100 as *mut u32;
    let nvic_base = 0xe000_e000 as *mut u32;
    let nvic_iser0 = nvic_base.wrapping_byte_add(0x100);
    let rcc_ahbenr = rcc_base.wrapping_byte_add(0x14);
    let rcc_apb2enr = rcc_base.wrapping_byte_add(0x18);
    let gpio_a_moder = gpio_a_base.wrapping_byte_add(0x00);
    let gpio_a_pupdr = gpio_a_base.wrapping_byte_add(0x0c);
    let gpio_a_afrh = gpio_a_base.wrapping_byte_add(0x24);
    let syscfg_exticr1 = syscfg_base.wrapping_byte_add(0x08);
    let exti_imr1 = exti_base.wrapping_byte_add(0x00);
    let exti_rtsr1 = exti_base.wrapping_byte_add(0x08);
    let usart1_cr1 = USART1_BASE.wrapping_byte_add(0x00);
    //let usart1_cr2 = USART1_BASE.wrapping_byte_add(0x04);
    let usart1_brr = USART1_BASE.wrapping_byte_add(0x0C);

    // name the values
    let rcc_gpio_a_en_bit = (1 << 17) as u32;
    let rcc_syscfg_en_bit = 0b1_u32;
    let gpio_a_0_mode_mask = 0b11_u32;
    let gpio_a_0_mode = 0b00_u32;
    let gpio_a_0_pupdr_mask = 0b11_u32;
    let gpio_a_0_pupd = 0b10_u32;
    let syscfg_exticr1_exti0_mask = 0b1111_u32;
    let syscfg_exticr1_exti0_pa0 = 0b000_u32; // bit 3 is reserved, lowest 3 bits is meaningful
    let exti_imr1_pa0_bit = 0b1_u32;
    let exti_rtsr1_pa0_bit = 0b1_u32;
    let nvic_iser0_exti0_mask = 1 << 6; // exti0 interrupt is interrupt 6

    // known, fixed values, lets us check our work with byte oriented registers
    //let _b1 = unsafe { ptr::read_volatile(0xe000_ed00 as *mut u8) };
    //let _b2 = unsafe { ptr::read_volatile(0xe000_ed01 as *mut u8) };
    //let _b3 = unsafe { ptr::read_volatile(0xe000_ed02 as *mut u8) };
    //let _b4 = unsafe { ptr::read_volatile(0xe000_ed03 as *mut u8) };

    // set up IO PA0 interrupt, connected to the USER button on the board
    unsafe {
        asm!("cpsie i"); // enable interrupts

        // enable EXTI0 interrupt in NVIC
        ptr::write_volatile(
            nvic_iser0,
            ptr::read_volatile(nvic_iser0) | nvic_iser0_exti0_mask,
        );

        // enable GPIOA clock
        ptr::write_volatile(
            rcc_ahbenr,
            ptr::read_volatile(rcc_ahbenr) | rcc_gpio_a_en_bit,
        );

        // enable SYSCFG clock
        ptr::write_volatile(
            rcc_apb2enr,
            ptr::read_volatile(rcc_apb2enr) | rcc_syscfg_en_bit,
        );

        // set PA0 to input
        ptr::write_volatile(
            gpio_a_moder,
            (ptr::read_volatile(gpio_a_moder) & !gpio_a_0_mode_mask) | gpio_a_0_mode,
        );

        // set PA0 pull-downs
        ptr::write_volatile(
            gpio_a_pupdr,
            (ptr::read_volatile(gpio_a_pupdr) & !gpio_a_0_pupdr_mask) | gpio_a_0_pupd,
        );

        // set EXTI0 input to PA0
        ptr::write_volatile(
            syscfg_exticr1,
            (ptr::read_volatile(syscfg_exticr1) & !syscfg_exticr1_exti0_mask)
                | syscfg_exticr1_exti0_pa0,
        );

        // enable EXTI0 interrupt
        ptr::write_volatile(exti_imr1, ptr::read_volatile(exti_imr1) | exti_imr1_pa0_bit);

        // enable rising edge interrupt for EXTI0
        ptr::write_volatile(
            exti_rtsr1,
            ptr::read_volatile(exti_rtsr1) | exti_rtsr1_pa0_bit,
        );
    }

    // set up USART1
    unsafe {
        // alternate function mode
        let gpio_a_9_10_moder_mask = 0b11_11_11_11_11_00_00_11_11_11_11_11_11_11_11_11_u32;
        let gpio_a_9_10_moder_mode = 0b00_00_00_00_00_10_10_00_00_00_00_00_00_00_00_00_u32;

        // pull down on RX line/PA10
        let gpio_a_9_pupdr_mask = 0b11_11_11_11_11_00_11_11_11_11_11_11_11_11_11_11_u32;
        let gpio_a_9_pupdr = 0b00_00_00_00_00_10_00_00_00_00_00_00_00_00_00_00_u32;

        let gpio_a_9_10_afrh_mask = 0b1111_1111_1111_1111_1111_0000_0000_1111_u32;
        let gpio_a_9_10_afrh = 0b0000_0000_0000_0000_0000_0000_0111_0111_u32;

        ptr::write_volatile(
            gpio_a_moder,
            (ptr::read_volatile(gpio_a_moder) & gpio_a_9_10_moder_mask) | gpio_a_9_10_moder_mode,
        );

        // set pull ups/pulldowns for USART1
        ptr::write_volatile(
            gpio_a_pupdr,
            (ptr::read_volatile(gpio_a_pupdr) & gpio_a_9_pupdr_mask) | gpio_a_9_pupdr,
        );

        // set alternate function modes to 7, USART1 RX/TX
        ptr::write_volatile(
            gpio_a_afrh,
            (ptr::read_volatile(gpio_a_afrh) & gpio_a_9_10_afrh_mask) | gpio_a_9_10_afrh,
        );

        // enable clock to USART1
        ptr::write_volatile(
            rcc_apb2enr,
            ptr::read_volatile(rcc_apb2enr) | ((1 << 14) as u32),
        );

        // TX/RX/USART enable
        ptr::write_volatile(usart1_cr1, ptr::read_volatile(usart1_cr1) | 0b1101_u32);

        // 115200 baud, assuming 8 MHz HSI clock is being used
        ptr::write_volatile(usart1_brr, (8_000_000 / 115200) as u32);

        //// auto baud rate enable
        //ptr::write_volatile(
        //    usart1_cr2,
        //    ptr::read_volatile(usart1_cr2) | ((1 << 20) as u32),
        //);
    }

    unsafe {
        loop {
            asm!("nop");
            //asm!("wfi"); // wait for interrupt
            //COUNTER.store(COUNTER.load(Ordering::Relaxed) + 1, Ordering::Relaxed);

            // TODO: loop should write a repeating sequence of characters to USART1, verify in minicom
        }
    }
}

#[panic_handler]
fn panic_handler(_ctx: &core::panic::PanicInfo) -> ! {
    loop {}
}

#[repr(C)]
pub union NVIC_Vector {
    handler: unsafe extern "C" fn(),
    reserved: u32,
}

// this interrupts table is placed at an offset of 0x40 within the NVIC_Vector table
// 0x40 == 64 == 16 * 4, this starts at index 16, which is this Cortex M4's defined IRQn 0
// 4 byte of alignment on a 32 bit mcu
// this is done because the first 16 vectors are reserved for the cortex
#[unsafe(link_section = ".vector_table.interrupts")]
#[unsafe(no_mangle)]
pub static __INTERRUPTS: [NVIC_Vector; 85] = [
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: EXTI0_Handler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
    NVIC_Vector {
        handler: DefaultHandler,
    },
];
