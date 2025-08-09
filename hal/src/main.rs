#![no_std]
#![no_main]

// pick a panicking behavior
// use panic_halt as _; // you can put a breakpoint on `rust_begin_unwind` to catch panics
// use panic_abort as _; // requires nightly
// use panic_itm as _; // logs messages over ITM; requires ITM support
use panic_semihosting as _; // logs messages to the host stderr; requires a debugger

use core::cell::RefCell;
use cortex_m::{asm, peripheral::NVIC};
use cortex_m_rt::entry;
use cortex_m_semihosting::hprintln;
use critical_section::Mutex;
use stm32f3::stm32f303::{Peripherals, interrupt};
use stm32f3::stm32f303::USART1;
use stm32f3::stm32f303::EXTI;

static EXTI: Mutex<RefCell<Option<EXTI>>> = Mutex::new(RefCell::new(None));
static USART: Mutex<RefCell<Option<USART1>>> = Mutex::new(RefCell::new(None));

#[entry]
fn main() -> ! {
    let peripherals = Peripherals::take().unwrap();

    // enable GPIOA/GPIOE peripheral clocks
    peripherals
        .RCC
        .ahbenr()
        .modify(|_, w| w.iopaen().bit(true).iopeen().bit(true));

    // enable USART1 peripheral clock
    peripherals
        .RCC
        .apb2enr()
        .modify(|_, w| w.usart1en().bit(true));

    // set PA9/10 to alternate function mode
    // set PA0 to input
    peripherals.GPIOA.moder().modify(unsafe { |_, w| {
        w.moder10()
            .bits(0b10)
            .moder9()
            .bits(0b10)
            .moder0()
            .bits(0b00)
    }});

    // set pa9 as a push-pull output
    peripherals.GPIOA.otyper().modify(|_, w| w.ot9().bit(false));

    // set pull downs for PA10, disable for PA9, pull down PA0
    unsafe {
        peripherals.GPIOA.pupdr().modify(|_, w| {
            w.pupdr10()
                .bits(0b10)
                .pupdr9()
                .bits(0b00)
                .pupdr0()
                .bits(0b10)
        })
    };

    // set alternate functions for PA9/PA10 to USART TX/RX
    peripherals
        .GPIOA
        .afrh()
        .modify(unsafe { |_, w| w.afrh10().bits(0b0111).afrh9().bits(0b0111) });

    // set PA0 function to GPIO
    peripherals.GPIOA.afrl().modify(unsafe { |_, w| w.afrl0().bits(0b0000) });

    // set USART1 baud rate for 8MHz default clock to 115200
    peripherals
        .USART1
        .brr()
        .modify(unsafe { |_, w| w.brr().bits((8_000_000 / 115_200) as u16)});

    // transmitter enable, receiver enable, usart enable
    peripherals
        .USART1
        .cr1()
        .modify(|_, w| w.te().bit(true).re().bit(true).ue().bit(true));

    // set exti0 interrupt source to PA0
    peripherals
        .SYSCFG
        .exticr1()
        .modify(unsafe { |_, w| w.exti0().bits(0b0000) });

    // enable interrupts, events
    peripherals.EXTI.imr1().modify(|_, w| w.mr0().bit(true));
    peripherals.EXTI.emr1().modify(|_, w| w.mr0().bit(true));

    // enable rising edge interrupt for exti0
    peripherals.EXTI.rtsr1().modify(|_, w| w.tr0().bit(true));

    // auto baud rate enable
    //peripherals.USART1.cr2.modify(|_, w| w.abren().bit(true));

    //let mut gpioe = peripherals.GPIOE.split(&mut rcc.ahb);

    //let mut led = gpioe
    //    .pe8
    //    .into_push_pull_output(&mut gpioe.moder, &mut gpioe.otyper);

    unsafe { NVIC::unmask(interrupt::EXTI0) }

    critical_section::with(|cs| {
        *USART.borrow(cs).borrow_mut() = Some(peripherals.USART1);
        *EXTI.borrow(cs).borrow_mut() = Some(peripherals.EXTI);
    });

    loop {
        asm::wfi();

        hprintln!("awoken");
    }
}

#[interrupt]
fn EXTI0() {
    critical_section::with(|cs| {
        EXTI.borrow(cs)
            .borrow_mut()
            .as_mut()
            .unwrap()
            .pr1()
            .modify(|_, w| w.pr0().bit(true));

        USART
            .borrow(cs)
            .borrow_mut()
            .as_mut()
            .unwrap()
            .tdr()
            // '$' in ascii
            .modify(unsafe { |_, w| w.tdr().bits(0x24) });
    });
}

