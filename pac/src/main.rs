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
//use cortex_m_semihosting::hprintln;
use critical_section::Mutex;
use stm32f3xx_hal::gpio;
use stm32f3xx_hal::gpio::{Edge, GpioExt, Input};
use stm32f3xx_hal::interrupt;
use stm32f3xx_hal::pac;
use stm32f3xx_hal::pac::USART1;
//use stm32f3xx_hal::prelude::_embedded_hal_digital_ToggleableOutputPin;
use stm32f3xx_hal::prelude::_stm32f3xx_hal_rcc_RccExt;
use stm32f3xx_hal::prelude::_stm32f3xx_hal_syscfg_SysCfgExt;

static BUTTON: Mutex<RefCell<Option<gpio::PA0<Input>>>> = Mutex::new(RefCell::new(None));
static USART: Mutex<RefCell<Option<USART1>>> = Mutex::new(RefCell::new(None));

#[entry]
fn main() -> ! {
    let peripherals = pac::Peripherals::take().unwrap();

    // enable GPIOA/GPIOE peripheral clocks
    peripherals
        .RCC
        .ahbenr
        .modify(|_, w| w.iopaen().bit(true).iopeen().bit(true));

    // enable USART1 peripheral clock
    peripherals
        .RCC
        .apb2enr
        .modify(|_, w| w.usart1en().bit(true));

    // set PA9/10 to alternate function mode
    peripherals
        .GPIOA
        .moder
        .modify(|_, w| w.moder10().bits(0b10).moder9().bits(0b10));

    // set pa9 as an output push-pull
    peripherals.GPIOA.otyper.modify(|_, w| w.ot9().bit(false));
    // set pa9 as an output open-drain
    //peripherals.GPIOA.otyper.modify(|_, w| w.ot9().bit(true));

    // set pull downs for PA10
    unsafe {
        peripherals
            .GPIOA
            .pupdr
            .modify(|_, w| w.pupdr10().bits(0b10).pupdr9().bits(0b00))
    };

    // set alternate functions for PA9/PA10 to USART TX/RX
    peripherals
        .GPIOA
        .afrh
        .modify(|_, w| w.afrh10().bits(0b0111).afrh9().bits(0b0111));

    // set USART1 baud rate for 8MHz default clock to 115200
    peripherals
        .USART1
        .brr
        .modify(|_, w| w.brr().bits((8_000_000 / 115_200) as u16));

    // transmitter enable, receiver enable, usart enable
    peripherals
        .USART1
        .cr1
        .modify(|_, w| w.te().bit(true).re().bit(true).ue().bit(true));

    // auto baud rate enable
    //peripherals.USART1.cr2.modify(|_, w| w.abren().bit(true));

    let mut rcc = peripherals.RCC.constrain();
    let mut exti = peripherals.EXTI;
    let mut syscfg = peripherals.SYSCFG.constrain(&mut rcc.apb2);
    let mut gpioa = peripherals.GPIOA.split(&mut rcc.ahb);
    //let mut gpioe = peripherals.GPIOE.split(&mut rcc.ahb);

    //let mut led = gpioe
    //    .pe8
    //    .into_push_pull_output(&mut gpioe.moder, &mut gpioe.otyper);

    let mut button = gpioa
        .pa0
        .into_pull_down_input(&mut gpioa.moder, &mut gpioa.pupdr);

    syscfg.select_exti_interrupt_source(&button);
    button.trigger_on_edge(&mut exti, Edge::Rising);
    button.enable_interrupt(&mut exti);

    unsafe { NVIC::unmask(button.interrupt()) }

    critical_section::with(|cs| {
        *BUTTON.borrow(cs).borrow_mut() = Some(button);

        *USART.borrow(cs).borrow_mut() = Some(peripherals.USART1)
    });

    loop {
        asm::wfi();

        //hprintln!("awoken");
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

        USART
            .borrow(cs)
            .borrow_mut()
            .as_mut()
            .unwrap()
            .tdr
            // '$' in ascii
            .modify(|_, w| w.tdr().bits(0x24));
    });
}

