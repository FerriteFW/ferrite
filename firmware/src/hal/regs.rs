//! Register addresses and bits the HAL uses. Mirrors fw/hal/at32_regs.h; grows as drivers are added.
#![allow(dead_code)] // the full set of bits, as in the C header

// Clock and reset (CRM)
pub const CRM: u32 = 0x4002_1000;
pub const CRM_CTRL: u32 = CRM;
pub const CRM_CFG: u32 = CRM + 0x04;
pub const CRM_APB1RST: u32 = CRM + 0x10;
pub const CRM_APB2EN: u32 = CRM + 0x18;
pub const CRM_APB1EN: u32 = CRM + 0x1C;
pub const CRM_CTRLSTS: u32 = CRM + 0x24;
pub const CRM_CTRL_HICKEN: u32 = 1 << 0;
pub const CRM_CTRL_HICKSTBL: u32 = 1 << 1;
pub const CRM_CTRL_HEXTEN: u32 = 1 << 16;
pub const CRM_CTRL_HEXTSTBL: u32 = 1 << 17;
pub const CRM_CTRL_PLLEN: u32 = 1 << 24;
pub const CRM_CTRL_PLLSTBL: u32 = 1 << 25;
pub const CRM_CFG_SCLKSEL_MASK: u32 = 3 << 0;
pub const CRM_CFG_SCLKSTS_MASK: u32 = 3 << 2; // 0 = HICK
pub const CRM_CFG_DIV_MASK: u32 = (0xF << 4) | (7 << 8) | (7 << 11); // AHB, APB1, APB2
pub const CRM_CFG_ADCDIV_MASK: u32 = (3 << 14) | (1 << 28); // 0 = APB2 / 2
pub const CRM_CTRLSTS_RSTFC: u32 = 1 << 24; // write 1: clear reset flags
pub const CRM_APB1_TMR2: u32 = 1 << 0;
pub const CRM_APB1_TMR3: u32 = 1 << 1;
pub const CRM_APB1_TMR4: u32 = 1 << 2;
pub const CRM_APB1_TMR5: u32 = 1 << 3;
pub const CRM_APB1_TMR6: u32 = 1 << 4;
pub const CRM_APB1_TMR7: u32 = 1 << 5;
pub const CRM_APB1_SPI2: u32 = 1 << 14;
pub const CRM_APB1_USART3: u32 = 1 << 18;
pub const CRM_APB1_UART4: u32 = 1 << 19;
pub const CRM_APB1_DAC: u32 = 1 << 29; // same bit in APB1RST and APB1EN
pub const CRM_APB2_IOMUX: u32 = 1 << 0;
pub const CRM_APB2_GPIOA_E: u32 = 0x1F << 2;
pub const CRM_APB2_ADC1: u32 = 1 << 9;
pub const CRM_APB2_TMR8: u32 = 1 << 13;
pub const CRM_APB2_USART1: u32 = 1 << 14;

// GPIO
pub const GPIO_CFGLR: u32 = 0x00;
pub const GPIO_CFGHR: u32 = 0x04;
pub const GPIO_IDT: u32 = 0x08;
pub const GPIO_ODT: u32 = 0x0C;
pub const GPIO_SCR: u32 = 0x10; // bits 0-15 set, bits 16-31 clear, atomic
pub const GPIO_CLR: u32 = 0x14;

// IOMUX (AFIO-compatible)
pub const IOMUX_REMAP: u32 = 0x4001_0004;
pub const IOMUX_SWJTAG_MASK: u32 = 7 << 24;
pub const IOMUX_SWJTAG_SWD_ONLY: u32 = 2 << 24; // JTAG off (frees PB3, PB4, PA15), SWD kept
/// TMR2_MUX (bits 9-8): 01 = CH1/EXT on PA15, CH2 on PB3, CH3 PA2, CH4 PA3 (RM_AT32F403A_407 V2.07, 7.4.2 IOMUX_REMAP).
pub const IOMUX_TMR2_MUX_MASK: u32 = 3 << 8;
pub const IOMUX_TMR2_MUX_PB3: u32 = 1 << 8;

// USART / UART
pub const USART1: u32 = 0x4001_3800;
pub const USART3: u32 = 0x4000_4800;
pub const UART4: u32 = 0x4000_4C00;
pub const USART_STS: u32 = 0x00;
pub const USART_DT: u32 = 0x04;
pub const USART_BAUDR: u32 = 0x08;
pub const USART_CTRL1: u32 = 0x0C;
pub const USART_STS_TDBE: u32 = 1 << 7; // transmit data buffer empty
pub const USART_STS_TDC: u32 = 1 << 6; // transmit data complete: the last stop bit has gone
pub const USART_STS_RDBF: u32 = 1 << 5; // receive data buffer full
pub const USART_STS_ERR: u32 = (1 << 3) | (1 << 2) | (1 << 1); // overrun, noise, framing
pub const USART_STS_ROERR: u32 = 1 << 3; // receiver overflow (overrun)
pub const USART_STS_FERR: u32 = 1 << 1; // framing error
pub const USART_CTRL1_UEN: u32 = 1 << 13;
pub const USART_CTRL1_TEN: u32 = 1 << 3;
pub const USART_CTRL1_REN: u32 = 1 << 2;
pub const USART_CTRL1_RDBFIEN: u32 = 1 << 5;
pub const USART_CTRL1_TDBEIEN: u32 = 1 << 7;

// Cortex-M4 core
pub const SYST_CSR: u32 = 0xE000_E010;
pub const NVIC_ISER: u32 = 0xE000_E100;
pub const NVIC_ICER: u32 = 0xE000_E180;
pub const NVIC_ICPR: u32 = 0xE000_E280;
pub const NVIC_IPR: u32 = 0xE000_E400; // one byte per interrupt
pub const SCB_CPUID: u32 = 0xE000_ED00;
pub const SCB_AIRCR: u32 = 0xE000_ED0C;
pub const SCB_SCR: u32 = 0xE000_ED10;
pub const SCB_SCR_SEVONPEND: u32 = 1 << 4;
pub const DEMCR: u32 = 0xE000_EDFC;
pub const DWT_CTRL: u32 = 0xE000_1000;
pub const DWT_CYCCNT: u32 = 0xE000_1004;

// ADC1
pub const ADC1: u32 = 0x4001_2400;
pub const ADC_STS: u32 = ADC1;
pub const ADC_CTRL1: u32 = ADC1 + 0x04;
pub const ADC_CTRL2: u32 = ADC1 + 0x08;
pub const ADC_SPT1: u32 = ADC1 + 0x0C; // sample time, channels 10-17
pub const ADC_SPT2: u32 = ADC1 + 0x10; // sample time, channels 0-9
pub const ADC_OSQ1: u32 = ADC1 + 0x2C;
pub const ADC_OSQ3: u32 = ADC1 + 0x34;
pub const ADC_ODT: u32 = ADC1 + 0x4C;
pub const ADC_STS_OCCE: u32 = 1 << 1; // ordinary conversion complete
pub const ADC_CTRL2_ADCEN: u32 = 1 << 0;
pub const ADC_CTRL2_ADCAL: u32 = 1 << 2;
pub const ADC_CTRL2_ADCALINIT: u32 = 1 << 3;
pub const ADC_CTRL2_OCTESEL_SW: u32 = 7 << 17; // ordinary trigger: software
pub const ADC_CTRL2_OCTEN: u32 = 1 << 20;
pub const ADC_CTRL2_OCSWTRG: u32 = 1 << 22;
pub const ADC_CTRL2_ITSRVEN: u32 = 1 << 23; // temperature sensor and VREFINT on

// SPI2
pub const SPI2: u32 = 0x4000_3800;
pub const SPI_CTRL1: u32 = 0x00;
pub const SPI_STS: u32 = 0x08;
pub const SPI_DT: u32 = 0x0C;
pub const SPI_CTRL1_CLKPHA: u32 = 1 << 0;
pub const SPI_CTRL1_CLKPOL: u32 = 1 << 1;
pub const SPI_CTRL1_MSTEN: u32 = 1 << 2;
pub const SPI_CTRL1_SPIEN: u32 = 1 << 6;
pub const SPI_CTRL1_SWCSIL: u32 = 1 << 8;
pub const SPI_CTRL1_SWCSEN: u32 = 1 << 9;
pub const SPI_STS_RDBF: u32 = 1 << 0;
pub const SPI_STS_TDBE: u32 = 1 << 1;

// DAC (channel 1 = PA4 audio; channel 2 = PA5 PA drive, never enabled by this image)
pub const DAC: u32 = 0x4000_7400;
pub const DAC_CTRL: u32 = DAC;
pub const DAC_DHR12R1: u32 = DAC + 0x08;
pub const DAC_CTRL_EN1: u32 = 1 << 0;
pub const DAC_CTRL_TEN1: u32 = 1 << 2;
pub const DAC_CTRL_TSEL1_TMR6: u32 = 0 << 3;
pub const DAC_CTRL_WAVE1_TRIANGLE: u32 = 2 << 6;
pub const DAC_CTRL_CH2_MASK: u32 = 0xFFFF_0000;
/// Channel 2 (PA drive) enable and its 8-bit right-aligned data, as stock writes it (DHR8R2, `analysis/calibration.md`).
#[cfg(feature = "tx")]
pub const DAC_CTRL_EN2: u32 = 1 << 16;
#[cfg(feature = "tx")]
pub const DAC_DHR8R2: u32 = DAC + 0x1C;

// Timers (STM32F1 TIM layout): TMR2-5 general purpose, TMR6 / TMR7 basic, TMR8 advanced
pub const TMR2: u32 = 0x4000_0000;
pub const TMR3: u32 = 0x4000_0400;
pub const TMR4: u32 = 0x4000_0800;
pub const TMR5: u32 = 0x4000_0C00;
pub const TMR6: u32 = 0x4000_1000;
pub const TMR7: u32 = 0x4000_1400;
pub const TMR8: u32 = 0x4001_3400;
pub const TMR_CTRL1: u32 = 0x00;
pub const TMR_CTRL2: u32 = 0x04;
pub const TMR_IDEN: u32 = 0x0C; // interrupt enables
pub const TMR_ISTS: u32 = 0x10; // flags, write 0 to clear
pub const TMR_SWEVT: u32 = 0x14;
pub const TMR_CM1: u32 = 0x18;
pub const TMR_CM2: u32 = 0x1C;
pub const TMR_CCTRL: u32 = 0x20;
pub const TMR_CVAL: u32 = 0x24;
pub const TMR_DIV: u32 = 0x28;
pub const TMR_PR: u32 = 0x2C;
pub const TMR_C1DT: u32 = 0x34;
pub const TMR_C2DT: u32 = 0x38;
pub const TMR_C4DT: u32 = 0x40;
pub const TMR_BRK: u32 = 0x44;
pub const TMR_CTRL2_PTOS_OVF: u32 = 2 << 4; // TRGO on overflow
pub const TMR_BRK_OEN: u32 = 1 << 15; // main output enable (advanced timers)

// Device identity
pub const DEV_FLASH_KB: u32 = 0x1FFF_F7E0;
pub const DEV_UID: u32 = 0x1FFF_F7E8;
pub const DEBUG_IDCODE: u32 = 0xE004_2000; // AT32 PID: RM 27.4.1; same address on STM32/GD32

// Flash controller and user system data (option bytes)
pub const FLASH_USD: u32 = 0x4002_201C;
pub const FLASH_USD_FAP: u32 = 1 << 1; // flash access protection active
pub const USD_FAP: u32 = 0x1FFF_F800;
pub const USD_SSB: u32 = 0x1FFF_F802;
pub const USD_EOPB0: u32 = 0x1FFF_F810; // AT32F403A: SRAM size option
