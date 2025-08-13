#![no_std]
#![no_main]

// pick a panicking behavior
// use panic_halt as _; // you can put a breakpoint on `rust_begin_unwind` to catch panics
// use panic_abort as _; // requires nightly
// use panic_itm as _; // logs messages over ITM; requires ITM support
use panic_semihosting as _; // logs messages to the host stderr; requires a debugger

use core::default;
use embassy_stm32::{
    exti::ExtiInput,
    gpio::{Level, Output, Pull, Speed},
    mode::Blocking,
    usart,
    usart::Uart,
};

#[embassy_executor::task]
async fn buttoner(
    mut button: ExtiInput<'static>,
    mut led: Output<'static>,
    mut usart: Uart<'static, Blocking>,
) {
    loop {
        button.wait_for_rising_edge().await;
        led.toggle();
        usart.blocking_write(&[0x24]).unwrap();
    }
}

#[embassy_executor::main]
async fn main(spawner: embassy_executor::Spawner) {
    let peripherals = embassy_stm32::init(Default::default());

    let button = ExtiInput::new(peripherals.PA0, peripherals.EXTI0, Pull::Down);
    let led = Output::new(peripherals.PE8, Level::Low, Speed::Low);
    let mut usart_config = <usart::Config as default::Default>::default();

    usart_config.baudrate = 115200;

    let usart = Uart::new_blocking(
        peripherals.USART1,
        peripherals.PA10,
        peripherals.PA9,
        usart_config,
    )
    .unwrap();

    spawner.spawn(buttoner(button, led, usart)).unwrap();
}
