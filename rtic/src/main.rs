#![no_std]
#![no_main]

// use panic_halt as _; // you can put a breakpoint on `rust_begin_unwind` to catch panics
// use panic_abort as _; // requires nightly
// use panic_itm as _; // logs messages over ITM; requires ITM support
use panic_semihosting as _; // logs messages to the host stderr; requires a debugger

//#[rtic::app(device = stm32f3xx_hal::pac, peripherals = true, dispatchers = [SPI1])]
#[rtic::app(device = stm32f3xx_hal::pac, dispatchers = [SPI1])]
mod app {
    use cortex_m_semihosting::hprintln;
    use stm32f3xx_hal::gpio::{AF7, Edge::Rising, Input, Output, PA0, PA9, PA10, PE8, PushPull};
    use stm32f3xx_hal::pac::USART1;
    use stm32f3xx_hal::prelude::*;
    use stm32f3xx_hal::serial::Serial;

    type SerialType = Serial<USART1, (PA9<AF7<PushPull>>, PA10<AF7<PushPull>>)>;

    #[shared]
    struct Shared {
        // these fields could be #[lock_free] because no other task uses them
        usart: SerialType,
        led: PE8<Output<PushPull>>,
        button: PA0<Input>,
    }

    #[local]
    struct Local {}

    #[init]
    fn init(cx: init::Context) -> (Shared, Local) {
        hprintln!("init");

        let peripherals = cx.device;
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

        let usart = Serial::new(
            peripherals.USART1,
            (pa9, pa10),
            115200.Bd(),
            clocks,
            &mut rcc.apb2,
        );

        (Shared { usart, led, button }, Local {})
    }

    #[idle]
    fn idle(_: idle::Context) -> ! {
        loop {
            hprintln!("running");
        }
    }

    //// using locals instead of shared
    //#[task(binds = EXTI0, local = [led, button, usart])]
    //fn button_handler_local(cx: button_handler_local::Context) {
    //    cx.local.button.clear_interrupt();
    //    cx.local.led.toggle().unwrap();
    //    cx.local.usart.write(b'$').unwrap();
    //    //hprintln!("tasking");
    //}

    #[task(binds = EXTI0, shared = [led, button, usart])]
    fn button_handler_shared(cx: button_handler_shared::Context) {
        let button = cx.shared.button;
        let led = cx.shared.led;
        let usart = cx.shared.usart;

        (button, led, usart).lock(|b, l, u| {
            b.clear_interrupt();
            l.toggle().unwrap();
            u.write(b'$').unwrap();
        });
    }
}

