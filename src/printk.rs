/*! Derived from https://github.com/kennystrawnmusic/printk */

use x86_64::instructions::interrupts::without_interrupts;

#[allow(unused_imports)]
use {
    bootloader_api::{
        info::{
            FrameBufferInfo,
            PixelFormat,
        }
    },
    conquer_once::{
        spin::{
            OnceCell,
        }
    },
    spin::RwLock,
    core::{
        fmt::{
            self,
            Write,
        },
        ptr,
    },
    noto_sans_mono_bitmap::{
        get_raster,
        get_raster_width,
        RasterizedChar,
        RasterHeight,
        FontWeight,
    },
};

/// Memory safety: need to ensure that each instance is mutexed
pub struct LockedPrintk(RwLock<Printk>);

impl LockedPrintk {
    
    // Constructor
    #[allow(dead_code)]
    pub fn new(buf: &'static mut [u8], i: FrameBufferInfo) -> Self {
        LockedPrintk(RwLock::new(Printk::new(buf, i)))
    }

}

impl log::Log for LockedPrintk {

    fn enabled(&self, _metadata: &log::Metadata) -> bool {
        true
    }

    fn log(&self, record: &log::Record) {
	without_interrupts(|| {
            let mut printk = self.0.write();
            writeln!(printk, "{}", record.args()).unwrap();
            printk.move_down(2);
	});
    }

    fn flush(&self) {
        
    }
}

/// Structure to render characters to the framebuffer
pub struct Printk {
    buffer: &'static mut [u8],
    info: FrameBufferInfo,
    x: usize,
    y: usize,
}

impl Printk {
    /// Creates a new empty logging interface
    #[allow(dead_code)]
    pub fn new(buffer: &'static mut [u8], info: FrameBufferInfo) -> Self {
        let mut printk = Self {
            buffer,
            info,
            x: 0,
            y: 0,
        };
        printk.clear();
        printk
    }

    /// Draws black-and-white pixels on the screen
    pub fn draw_grayscale(&mut self, x: usize, y: usize, intensity: u8) {

        // Pixel offset
        let poff = y * self.info.stride + x;

        let u8_intensity = {
            if intensity > 200 {
                0xf
            } else {
                0
            }
        };

        let color = match self.info.pixel_format {

            PixelFormat::Rgb => { 
                [intensity, intensity, intensity/2, 0]
            },

            PixelFormat::Bgr => {
                [intensity/2, intensity, intensity, 0]
            },

            PixelFormat::U8 => {
                [u8_intensity, 0, 0, 0]
            },

            //TODO: use embedded-graphics to solve this problem
            _ => panic!("Unknown pixel format")
        };

        // Number of bytes in a pixel (4 on my machine)
        let bpp = self.info.bytes_per_pixel;

        // Byte offset: multiply bytes-per-pixel by pixel offset to obtain
        let boff = poff*bpp;

        // Copy bytes
        self.buffer[boff..(boff+bpp)].copy_from_slice(&color[..bpp]);

        // Raw pointer to buffer start ― that's why this is unsafe
        let _ = unsafe { ptr::read_volatile(&self.buffer[boff]) };

    }

    /// Renders characters from the `noto-sans-mono-bitmap` crate
    pub fn render(&mut self, rendered: RasterizedChar) {
        
        // Loop through lines
        for (y, ln) in rendered.raster().iter().enumerate() {

            // Loop through characters on each line
            for (x, col) in ln.iter().enumerate() {

                // Use above draw_grayscale method to render each character in the bitmap
                self.draw_grayscale(self.x+x, self.y+y, *col)
            }
        }

        // Increment by width of each character
        self.x += rendered.width();
    }

    /// Moves down by `distance` number of pixels
    pub fn move_down(&mut self, distance: usize) {
        self.y += distance;
    }

    /// Moves to the beginning of a line
    pub fn home(&mut self) {
        self.x = 0;
    }

    /// Moves down one line
    pub fn next_line(&mut self) {
        self.move_down(16);
        self.home();
    }

    /// Moves to the top of the page
    pub fn page_up(&mut self) {
        self.y = 0;
    }

    /// Clears the screen
    pub fn clear(&mut self) {
        self.home();
        self.page_up();
        self.buffer.fill(0);
    }

    /// Gets the width of the framebuffer
    pub fn width(&self) -> usize {
        self.info.width
    }

    /// Gets the height of the framebuffer
    pub fn height(&self) -> usize {
        self.info.height
    }


    /// Prints an individual character on the screen
    pub fn putch(&mut self, c: char) {
        match c {
            '\n' => self.next_line(),
            '\r' => self.home(),
            c => {
                if self.x >= self.width() {
                    self.next_line();

                }
                const LETTER_HEIGHT: usize = RasterHeight::Size16.val();

                if self.y >= (self.height() - LETTER_HEIGHT) {
                    self.clear();
                }

                match get_raster(c, FontWeight::Regular, RasterHeight::Size16) {
		    Some(s) => self.render(s),
		    None => self.putch('?'),
		};
            }
        }
    }
}

unsafe impl Send for Printk {}
unsafe impl Sync for Printk {}

impl fmt::Write for Printk {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for c in s.chars() {
            // prevent deadlocks
            without_interrupts(|| {
                self.putch(c)
            })
        }
        Ok(())
    }
}
