/*! Derived from https://github.com/kennystrawnmusic/printk */

use x86_64::instructions::interrupts::without_interrupts;

#[allow(unused_imports)]
use {
    limine::framebuffer::{
	Framebuffer,
	MemoryModel,
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
	slice,
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
    pub fn new(i: Framebuffer<'static>) -> Self {
        LockedPrintk(RwLock::new(Printk::new(i)))
    }

    pub fn clear(&self) {
	without_interrupts(|| {
            let mut printk = self.0.write();
            printk.clear();
	});
    }

    pub unsafe fn force_unlock(&self) {
	self.0.force_write_unlock()
    }

    pub fn write_str(&self, s: &str) {
	without_interrupts(|| {
            let mut printk = self.0.write();
	    write!(printk, "{}", s).unwrap();
	});
    }

    pub fn write_char(&self, s: char) {
	without_interrupts(|| {
            let mut printk = self.0.write();
	    write!(printk, "{}", s).unwrap();
	});
    }

    pub fn get_rows(&self) -> u8 {
	without_interrupts(|| {
            let mut printk = self.0.write();
	    (printk.height() / 16) as u8
	})
    }

    pub fn get_cols(&self) -> u8 {
	without_interrupts(|| {
            let mut printk = self.0.write();
	    (printk.width() / get_raster_width(FontWeight::Regular, RasterHeight::Size16)) as u8
	})
    }
}

impl log::Log for LockedPrintk {

    fn enabled(&self, _metadata: &log::Metadata) -> bool {
        true
    }

    fn log(&self, record: &log::Record) {
	unsafe { self.force_unlock(); }
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
    fb: Framebuffer<'static>,
    x: usize,
    y: usize,
}

impl Printk {
    /// Creates a new empty logging interface
    #[allow(dead_code)]
    pub fn new(fb: Framebuffer<'static>) -> Self {
        let mut printk = Self {
            fb,
            x: 0,
            y: 0,
        };
        printk.clear();
        printk
    }

    /// Draws black-and-white pixels on the screen
    pub fn draw_grayscale(&mut self, x: usize, y: usize, intensity: u8) {

        // Number of bytes in a pixel (4 on my machine)
        let bpp = self.fb.bpp() as usize / 8;

        // Pixel offset
        let poff = y * self.fb.pitch() as usize + (x * bpp);

        let color = match self.fb.memory_model() {
	    MemoryModel::RGB => { 
                [intensity, intensity, intensity/2, 0]
            },

            _ => panic!("Unknown pixel format")
        };

        // Copy bytes
	unsafe {
            slice::from_raw_parts_mut(self.fb.addr(), self.fb.height() as usize * self.fb.pitch() as usize)
		[poff .. poff+bpp].copy_from_slice(&color[..bpp]);
	}

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

	unsafe {
            slice::from_raw_parts_mut(self.fb.addr(), self.fb.height() as usize * self.fb.pitch() as usize).fill(0);
	}
    }

    /// Gets the width of the framebuffer
    pub fn width(&self) -> usize {
        self.fb.width() as usize
    }

    /// Gets the height of the framebuffer
    pub fn height(&self) -> usize {
        self.fb.height() as usize
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
