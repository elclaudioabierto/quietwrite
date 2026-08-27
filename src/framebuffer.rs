use fontdue::{Font, FontSettings, Metrics};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::ptr;
use std::sync::Arc;

const FBIOGET_VSCREENINFO: libc::c_ulong = 0x4600;
const FBIOGET_FSCREENINFO: libc::c_ulong = 0x4602;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct FbBitfield {
    offset: u32,
    length: u32,
    msb_right: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct FbVarScreeninfo {
    xres: u32,
    yres: u32,
    xres_virtual: u32,
    yres_virtual: u32,
    xoffset: u32,
    yoffset: u32,
    bits_per_pixel: u32,
    grayscale: u32,
    red: FbBitfield,
    green: FbBitfield,
    blue: FbBitfield,
    transp: FbBitfield,
    nonstd: u32,
    activate: u32,
    height: u32,
    width: u32,
    accel_flags: u32,
    pixclock: u32,
    left_margin: u32,
    right_margin: u32,
    upper_margin: u32,
    lower_margin: u32,
    hsync_len: u32,
    vsync_len: u32,
    sync: u32,
    vmode: u32,
    rotate: u32,
    colorspace: u32,
    reserved: [u32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct FbFixScreeninfo {
    id: [libc::c_char; 16],
    smem_start: libc::c_ulong,
    smem_len: u32,
    type_: u32,
    type_aux: u32,
    visual: u32,
    xpanstep: u16,
    ypanstep: u16,
    ywrapstep: u16,
    line_length: u32,
    mmio_start: libc::c_ulong,
    mmio_len: u32,
    accel: u32,
    capabilities: u16,
    reserved: [u16; 2],
}

struct Glyph {
    metrics: Metrics,
    bitmap: Arc<[u8]>,
}

#[derive(Clone, Copy)]
pub struct Layout {
    pub columns: usize,
    pub content_rows: usize,
    pub cell_width: usize,
    pub line_height: usize,
    pub margin_x: usize,
    pub margin_y: usize,
    pub logical_width: usize,
    pub logical_height: usize,
    pub font_size: f32,
    pub ascent: f32,
}

pub struct Framebuffer {
    _file: File,
    memory: *mut u8,
    memory_len: usize,
    width: usize,
    height: usize,
    stride: usize,
    bytes_per_pixel: usize,
    var: FbVarScreeninfo,
    font: Font,
    glyphs: HashMap<(char, u32), Glyph>,
}

impl Framebuffer {
    pub fn open() -> io::Result<Self> {
        let file = OpenOptions::new().read(true).write(true).open("/dev/fb0")?;
        let fd = file.as_raw_fd();
        let mut var = FbVarScreeninfo::default();
        let mut fix = FbFixScreeninfo::default();
        if unsafe { libc::ioctl(fd, FBIOGET_VSCREENINFO as _, &mut var) } != 0 {
            return Err(io::Error::last_os_error());
        }
        if unsafe { libc::ioctl(fd, FBIOGET_FSCREENINFO as _, &mut fix) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let bytes_per_pixel = (var.bits_per_pixel / 8) as usize;
        if !matches!(bytes_per_pixel, 2 | 4) {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!("unsupported framebuffer depth: {} bpp", var.bits_per_pixel),
            ));
        }
        let memory_len = fix.smem_len as usize;
        let memory = unsafe {
            libc::mmap(
                ptr::null_mut(),
                memory_len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd,
                0,
            )
        };
        if memory == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        let font = Font::from_bytes(
            include_bytes!("../assets/DejaVuSansMono.ttf").as_slice(),
            FontSettings::default(),
        )
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        Ok(Self {
            _file: file,
            memory: memory.cast(),
            memory_len,
            width: var.xres as usize,
            height: var.yres as usize,
            stride: fix.line_length as usize,
            bytes_per_pixel,
            var,
            font,
            glyphs: HashMap::new(),
        })
    }

    pub fn layout(&self, rotation: u8, zoom: i8) -> Layout {
        let (logical_width, logical_height) = if rotation % 2 == 0 {
            (self.width, self.height)
        } else {
            (self.height, self.width)
        };
        // Target about twelve text rows on the shortest screen axis. Users can adjust with F7/F8.
        let target_rows = (12_i16 - zoom as i16).clamp(7, 20) as f32;
        let font_size = (self.width.min(self.height) as f32 / target_rows).clamp(28.0, 112.0);
        let line_metrics = self.font.horizontal_line_metrics(font_size).unwrap();
        let line_height = line_metrics.new_line_size.ceil() as usize + 2;
        let cell_width = self.font.metrics('M', font_size).advance_width.ceil() as usize;
        let margin_x = (logical_width / 32).max(12);
        let margin_y = (logical_height / 40).max(8);
        let columns = logical_width
            .saturating_sub(margin_x * 2)
            .checked_div(cell_width.max(1))
            .unwrap_or(1)
            .max(1);
        let content_rows = logical_height
            .saturating_sub(margin_y * 2 + line_height * 2)
            .checked_div(line_height.max(1))
            .unwrap_or(1)
            .max(1);
        Layout {
            columns,
            content_rows,
            cell_width: cell_width.max(1),
            line_height,
            margin_x,
            margin_y,
            logical_width,
            logical_height,
            font_size,
            ascent: line_metrics.ascent,
        }
    }

    pub fn clear(&mut self, color: (u8, u8, u8), rotation: u8) {
        let (logical_width, logical_height) = if rotation % 2 == 0 {
            (self.width, self.height)
        } else {
            (self.height, self.width)
        };
        for y in 0..logical_height {
            for x in 0..logical_width {
                self.put_pixel(x, y, color, rotation);
            }
        }
    }

    pub fn rect(
        &mut self,
        x: usize,
        y: usize,
        width: usize,
        height: usize,
        color: (u8, u8, u8),
        rotation: u8,
    ) {
        for py in y..y.saturating_add(height) {
            for px in x..x.saturating_add(width) {
                self.put_pixel(px, py, color, rotation);
            }
        }
    }

    pub fn text(
        &mut self,
        mut x: usize,
        y: usize,
        text: &str,
        layout: Layout,
        foreground: (u8, u8, u8),
        background: (u8, u8, u8),
        rotation: u8,
    ) {
        let size_key = layout.font_size.round() as u32;
        for ch in text.chars() {
            if ch == '\t' {
                x += layout.cell_width * 4;
                continue;
            }
            let cell_count = super::char_width(ch, 0).max(1);
            let key = (ch, size_key);
            if !self.glyphs.contains_key(&key) {
                let (metrics, bitmap) = self.font.rasterize(ch, layout.font_size);
                self.glyphs.insert(
                    key,
                    Glyph {
                        metrics,
                        bitmap: bitmap.into(),
                    },
                );
            }
            let glyph = self.glyphs.get(&key).unwrap();
            let metrics = glyph.metrics;
            let bitmap = Arc::clone(&glyph.bitmap);
            let baseline = y as isize + layout.ascent.ceil() as isize;
            let top = baseline - metrics.height as isize - metrics.ymin as isize;
            let left = x as isize + metrics.xmin as isize;
            for gy in 0..metrics.height {
                for gx in 0..metrics.width {
                    let alpha = bitmap[gy * metrics.width + gx];
                    if alpha == 0 {
                        continue;
                    }
                    let px = left + gx as isize;
                    let py = top + gy as isize;
                    if px < 0 || py < 0 {
                        continue;
                    }
                    let blend = |front: u8, back: u8| -> u8 {
                        ((front as u16 * alpha as u16 + back as u16 * (255 - alpha) as u16) / 255)
                            as u8
                    };
                    self.put_pixel(
                        px as usize,
                        py as usize,
                        (
                            blend(foreground.0, background.0),
                            blend(foreground.1, background.1),
                            blend(foreground.2, background.2),
                        ),
                        rotation,
                    );
                }
            }
            x += layout.cell_width * cell_count;
        }
    }

    pub fn flush(&mut self) {
        unsafe {
            libc::msync(self.memory.cast(), self.memory_len, libc::MS_ASYNC);
        }
    }

    fn put_pixel(&mut self, x: usize, y: usize, color: (u8, u8, u8), rotation: u8) {
        let (physical_x, physical_y) = match rotation % 4 {
            0 => (x, y),
            1 => (self.width.saturating_sub(1).saturating_sub(y), x),
            2 => (
                self.width.saturating_sub(1).saturating_sub(x),
                self.height.saturating_sub(1).saturating_sub(y),
            ),
            _ => (y, self.height.saturating_sub(1).saturating_sub(x)),
        };
        if physical_x >= self.width || physical_y >= self.height {
            return;
        }
        let offset = physical_y * self.stride + physical_x * self.bytes_per_pixel;
        if offset + self.bytes_per_pixel > self.memory_len {
            return;
        }
        let pixel = channel(color.0, self.var.red)
            | channel(color.1, self.var.green)
            | channel(color.2, self.var.blue)
            | channel(255, self.var.transp);
        unsafe {
            if self.bytes_per_pixel == 2 {
                ptr::write_unaligned(self.memory.add(offset).cast::<u16>(), pixel as u16);
            } else {
                ptr::write_unaligned(self.memory.add(offset).cast::<u32>(), pixel);
            }
        }
    }
}

fn channel(value: u8, field: FbBitfield) -> u32 {
    if field.length == 0 {
        return 0;
    }
    let max = (1_u32 << field.length.min(8)) - 1;
    ((value as u32 * max / 255) & max) << field.offset
}

impl Drop for Framebuffer {
    fn drop(&mut self) {
        unsafe {
            libc::munmap(self.memory.cast(), self.memory_len);
        }
    }
}
