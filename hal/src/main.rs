#![no_std]
#![no_main]

// pick a panicking behavior
// use panic_halt as _; // you can put a breakpoint on `rust_begin_unwind` to catch panics
// use panic_abort as _; // requires nightly
// use panic_itm as _; // logs messages over ITM; requires ITM support
use panic_semihosting as _; // logs messages to the host stderr; requires a debugger

use core::cell::RefCell;
use cortex_m::asm;
use cortex_m::peripheral::NVIC;
use cortex_m_rt::entry;
use cortex_m_semihosting::hprintln;
use critical_section::Mutex;
use stm32f3xx_hal::interrupt;
use stm32f3xx_hal::pac;
use stm32f3xx_hal::pac::{EXTI, USART1};
use stm32f3xx_hal::prelude::*;
use stm32f3xx_hal::serial::Serial;
use stm32f3xx_hal::gpio::{PushPull, AF7, PA9, PA10};

type SerialType = Serial<USART1, (PA9<AF7<PushPull>>, PA10<AF7<PushPull>>)>;

static EXTI: Mutex<RefCell<Option<EXTI>>> = Mutex::new(RefCell::new(None));
static USART: Mutex<RefCell<Option<SerialType>>> = Mutex::new(RefCell::new(None));

#[entry]
fn main() -> ! {
    let peripherals = pac::Peripherals::take().unwrap();
    let mut rcc = peripherals.RCC.constrain();
    let mut flash = peripherals.FLASH.constrain();
    let clocks = rcc.cfgr.sysclk(48.MHz()).freeze(&mut flash.acr);
    let syscfg = peripherals.SYSCFG.constrain(&mut rcc.apb2);
    let mut gpioa = peripherals.GPIOA.split(&mut rcc.ahb);
    let mut gpioe = peripherals.GPIOE.split(&mut rcc.ahb);

    let _button = gpioa
        .pa0
        .into_pull_down_input(&mut gpioa.moder, &mut gpioa.pupdr);

    let _led = gpioe
        .pe8
        .into_push_pull_output(&mut gpioe.moder, &mut gpioe.otyper);

    // 7 is USART1 alternate function/af
    let pa9 =
        gpioa
            .pa9
            .into_af_push_pull::<7>(&mut gpioa.moder, &mut gpioa.otyper, &mut gpioa.afrh);

    let pa10 =
        gpioa
            .pa10
            .into_af_push_pull::<7>(&mut gpioa.moder, &mut gpioa.otyper, &mut gpioa.afrh);

    let usart_pins = (pa9, pa10);
    let usart1 = Serial::new(
        peripherals.USART1,
        usart_pins,
        115200.Bd(),
        clocks,
        &mut rcc.apb2,
    );

    // set exti0 interrupt source to PA0
    syscfg
        .exticr1
        .modify(unsafe { |_, w| w.exti0().bits(0b0000) });

    // enable interrupts, events
    peripherals.EXTI.imr1.modify(|_, w| w.mr0().bit(true));
    peripherals.EXTI.emr1.modify(|_, w| w.mr0().bit(true));

    // enable rising edge interrupt for exti0
    peripherals.EXTI.rtsr1.modify(|_, w| w.tr0().bit(true));

    //// auto baud rate enable
    ////peripherals.USART1.cr2.modify(|_, w| w.abren().bit(true));

    ////let mut gpioe = peripherals.GPIOE.split(&mut rcc.ahb);

    ////let mut led = gpioe
    ////    .pe8
    ////    .into_push_pull_output(&mut gpioe.moder, &mut gpioe.otyper);

    unsafe { NVIC::unmask(interrupt::EXTI0) }

    critical_section::with(|cs| {
        *USART.borrow(cs).borrow_mut() = Some(usart1);
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
            .pr1
            .modify(|_, w| w.pr0().bit(true));

        hprintln!("ahoy");

        USART
            .borrow(cs)
            .borrow_mut()
            .as_mut()
            .unwrap()
            .write(0x24)
            .unwrap();
    });
}

