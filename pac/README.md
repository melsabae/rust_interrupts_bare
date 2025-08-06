An application that intends to use almost no dependencies to stand up firmware on the STM32F3DISCOVERY board, which hosts an STM32F303VC chip.

Description of files:
    device.x: A mapping of external C functions to Rust functions. These are specifically targetting interrupts.
    memory.x: A memory layout script for the chip, ingested by the linker.
    build.rs: a rust program that copies a file. It is intended to trigger if/when memory.x is changed. It is not currently set up to do that, so if device.x or memory.x change, maybe run cargo clean and cargo build.
    Cargo.lock: automatically generated.
    Cargo.toml: the cargo config. Declares dependencies and their versions, and their features.
        The panic = "abort" section could potentially be removed if we pulled in the panic-halt crate, and used one of the provisions there for the panic_handler. I wanted to not use it for this case.
        The cortex-m-rt dependency is the only external dependency I used.
            I use the "entry" macro for the main function. It sets main as the program entry point.
            It also brings in some linker tools, which I consider an acceptable compromise, since I don't consider the linker stuff to be really part of the rust ecosystem.
            "device" feature enables manually setting the interrupt vector table. This feature mandates a device.x file.
            "set-sp" feature sets the stack pointer, which the memory.x file moves the stack to the CCM SRAM section on the chip. It's supposed to be faster and not DMA-able memory, so sounded good.
            "set-vtor" feature sets the VTOR pointer on the Cortex M4F chip. The vector table is currently part of the flash image, not in SRAM, so setting it to the address in the chip's flash section is required. However if it isn't set, I believe the chip re-maps it somewhere else, and it tended to work before using this feature. I'm keeping it.
    .cargo/config.toml: Some options to cargo, for cargo build/run. The runner is defined as GDB instead of QEMU, beacsue the intention is to now debug live on the chip. Additionally sets the target ABI, which induces cargo to choose a specific rustc version: namely one for the Cortex M4F.
    openocd.cfg: A configuration file when running OpenOCD to interact with the DISCOVERY ST-link debugger.
    openocd.gdb: A configuration file for GBD, that connects to the debugging session hosted by OpenOCD.
    src/main.rs: A chip program. It is almost purely written in regular rust.

