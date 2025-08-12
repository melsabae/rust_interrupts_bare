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
use stm32f3xx_hal::gpio::{AF7, Edge::Rising, Input, Output, PA0, PA9, PA10, PE8, PushPull};
use stm32f3xx_hal::interrupt;
use stm32f3xx_hal::pac;
use stm32f3xx_hal::pac::USART1;
use stm32f3xx_hal::prelude::*;
use stm32f3xx_hal::serial::Serial;

type SerialType = Serial<USART1, (PA9<AF7<PushPull>>, PA10<AF7<PushPull>>)>;

static USART: Mutex<RefCell<Option<SerialType>>> = Mutex::new(RefCell::new(None));
static LED: Mutex<RefCell<Option<PE8<Output<PushPull>>>>> = Mutex::new(RefCell::new(None));
static BUTTON: Mutex<RefCell<Option<PA0<Input>>>> = Mutex::new(RefCell::new(None));

#[entry]
fn main() -> ! {
    let peripherals = pac::Peripherals::take().unwrap();
    let mut rcc = peripherals.RCC.constrain();
    let mut flash = peripherals.FLASH.constrain();
    let mut exti = peripherals.EXTI;
    let clocks = rcc.cfgr.sysclk(48.MHz()).freeze(&mut flash.acr);
    let mut syscfg = peripherals.SYSCFG.constrain(&mut rcc.apb2);
    let mut gpioa = peripherals.GPIOA.split(&mut rcc.ahb);
    let mut gpioe = peripherals.GPIOE.split(&mut rcc.ahb);

    let mut button = gpioa
        .pa0
        .into_pull_down_input(&mut gpioa.moder, &mut gpioa.pupdr);

    syscfg.select_exti_interrupt_source(&button);
    button.trigger_on_edge(&mut exti, Rising);
    button.enable_interrupt(&mut exti);

    let led = gpioe
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

    let usart1 = Serial::new(
        peripherals.USART1,
        (pa9, pa10),
        115200.Bd(),
        clocks,
        &mut rcc.apb2,
    );

    unsafe { NVIC::unmask(button.interrupt()) }

    critical_section::with(|cs| {
        *BUTTON.borrow(cs).borrow_mut() = Some(button);
        *LED.borrow(cs).borrow_mut() = Some(led);
        *USART.borrow(cs).borrow_mut() = Some(usart1);
    });

    loop {
        asm::wfi();

        hprintln!("awoken");
    }
}

#[interrupt]
fn EXTI0() {
    critical_section::with(|cs| {
        BUTTON
            .borrow(cs)
            .borrow_mut()
            .as_mut()
            .unwrap()
            .clear_interrupt();

        LED.borrow(cs)
            .borrow_mut()
            .as_mut()
            .unwrap()
            .toggle()
            .unwrap();

        USART
            .borrow(cs)
            .borrow_mut()
            .as_mut()
            .unwrap()
            .write(0x24)
            .unwrap();
    });
}

