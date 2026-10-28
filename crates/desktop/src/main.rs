use std::borrow::Cow;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::Command;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use ab_glyph::{Font, FontVec, PxScale, ScaleFont};
use cosmic_text::{
    Attrs, Buffer, Color as TextColor, Family, FontSystem, Metrics, Shaping, Style, SwashCache,
    Weight, Wrap,
};
use ruterm_core::attributes::{Color, Flags};
use ruterm_core::terminal::{CursorShape, MouseMode, Terminal};
use ruterm_parser::Parser;
use ruterm_pty::{Pty, PtyOptions, PtySize};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, Ime, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey, PhysicalKey};
use winit::window::{Window, WindowAttributes, WindowId};

const COLS: usize = 80;
const ROWS: usize = 24;
const DEFAULT_FONT_SIZE: f32 = 12.0;
const FRAME_INTERVAL: Duration = Duration::from_millis(16);
const OUTPUT_BUFFER_LIMIT: usize = 256 * 1024;

#[derive(Clone, Debug, PartialEq)]
struct FontSettings {
    name: String,
    size: f32,
}

impl Default for FontSettings {
    fn default() -> Self {
        Self {
            name: "monospace".into(),
            size: DEFAULT_FONT_SIZE,
        }
    }
}

impl FontSettings {
    fn from_toml(contents: &str) -> Result<Self, String> {
        let config: toml::Value = contents.parse().map_err(|error| format!("{error}"))?;
        let mut settings = Self::default();
        if let Some(font) = config.get("font") {
            let font = font.as_table().ok_or("[font] must be a table")?;
            if let Some(name) = font.get("name") {
                settings.name = name
                    .as_str()
                    .ok_or("font.name must be a string")?
                    .to_owned();
                if settings.name.trim().is_empty() {
                    return Err("font.name must not be empty".into());
                }
            }
            if let Some(size) = font.get("size") {
                settings.size = size
                    .as_float()
                    .or_else(|| size.as_integer().map(|value| value as f64))
                    .ok_or("font.size must be a number")? as f32;
                if !settings.size.is_finite() || !(1.0..=128.0).contains(&settings.size) {
                    return Err("font.size must be between 1 and 128".into());
                }
            }
        }
        Ok(settings)
    }

    fn load() -> Self {
        let Some(home) = std::env::var_os("HOME") else {
            return Self::default();
        };
        let path = PathBuf::from(home).join(".config/ruterm/config.toml");
        match std::fs::read_to_string(&path) {
            Ok(contents) => match Self::from_toml(&contents) {
                Ok(settings) => settings,
                Err(error) => {
                    eprintln!("{}: {error}; using default font settings", path.display());
                    Self::default()
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(error) => {
                eprintln!("{}: {error}; using default font settings", path.display());
                Self::default()
            }
        }
    }
}

#[derive(Clone, Copy)]
struct CellSize {
    width: usize,
    height: usize,
}

impl CellSize {
    fn for_font(font: &FontVec, size: f32) -> Self {
        let scaled = font.as_scaled(PxScale::from(size));
        let advance = ['M', '0', 'W']
            .into_iter()
            .map(|ch| scaled.h_advance(font.glyph_id(ch)))
            .fold(0.0_f32, f32::max);
        Self {
            width: advance.ceil().max(1.0) as usize,
            height: (scaled.ascent() - scaled.descent() + scaled.line_gap())
                .ceil()
                .max(size.ceil()) as usize
                + 2,
        }
    }
}

enum UserEvent {
    Output,
    WriteError(String),
    End(String),
}

#[derive(Default)]
struct OutputBuffer {
    state: Mutex<OutputState>,
    space_available: Condvar,
}

#[derive(Default)]
struct OutputState {
    bytes: Vec<u8>,
    wake_pending: bool,
}

impl OutputBuffer {
    fn push(&self, bytes: &[u8]) -> bool {
        let mut state = self.state.lock().unwrap();
        while state.bytes.len() >= OUTPUT_BUFFER_LIMIT {
            state = self.space_available.wait(state).unwrap();
        }
        state.bytes.extend_from_slice(bytes);
        if state.wake_pending {
            false
        } else {
            state.wake_pending = true;
            true
        }
    }

    fn take(&self) -> Vec<u8> {
        let mut state = self.state.lock().unwrap();
        state.wake_pending = false;
        let bytes = std::mem::take(&mut state.bytes);
        self.space_available.notify_one();
        bytes
    }
}

struct Gpu {
    cell: CellSize,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    texture: wgpu::Texture,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    image_width: u32,
    image_height: u32,
    bind_group: wgpu::BindGroup,
    pipeline: wgpu::RenderPipeline,
}

impl Gpu {
    fn new(window: Arc<Window>, cell: CellSize) -> Result<Self, String> {
        let size = window.inner_size();
        // Some Vulkan drivers crash while compiling this simple pipeline. Use the
        // OpenGL backend on Linux so a driver failure cannot abort the process.
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::GL,
            ..Default::default()
        });
        let surface = instance.create_surface(window).map_err(|e| e.to_string())?;
        let adapter = futures_lite::future::block_on(instance.request_adapter(
            &wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            },
        ))
        .ok_or_else(|| "No compatible GPU adapter".to_string())?;
        let (device, queue) = futures_lite::future::block_on(
            adapter.request_device(&wgpu::DeviceDescriptor::default(), None),
        )
        .map_err(|e| e.to_string())?;
        let capabilities = surface.get_capabilities(&adapter);
        let format = capabilities
            .formats
            .iter()
            .copied()
            .find(wgpu::TextureFormat::is_srgb)
            .unwrap_or(capabilities.formats[0]);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::AutoNoVsync,
            alpha_mode: capabilities.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 1,
        };
        if size.width > 0 && size.height > 0 {
            surface.configure(&device, &config);
        }
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("terminal pixels"),
            size: wgpu::Extent3d {
                width: (COLS * cell.width) as u32,
                height: (ROWS * cell.height) as u32,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("terminal texture layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        multisampled: false,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("terminal texture bind group"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("terminal shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("terminal pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(format.into())],
                compilation_options: Default::default(),
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });
        Ok(Self {
            cell,
            surface,
            device,
            queue,
            config,
            texture,
            layout,
            sampler,
            image_width: (COLS * cell.width) as u32,
            image_height: (ROWS * cell.height) as u32,
            bind_group,
            pipeline,
        })
    }

    fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
    }

    fn resize_grid(&mut self, cols: usize, rows: usize) {
        self.image_width = (cols * self.cell.width) as u32;
        self.image_height = (rows * self.cell.height) as u32;
        self.texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("terminal pixels"),
            size: wgpu::Extent3d {
                width: self.image_width,
                height: self.image_height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = self
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        self.bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("terminal texture bind group"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
    }

    fn render(&mut self, pixels: &[u8]) -> Result<(), wgpu::SurfaceError> {
        if self.config.width == 0 || self.config.height == 0 {
            return Ok(());
        }
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(self.image_width * 4),
                rows_per_image: Some(self.image_height),
            },
            wgpu::Extent3d {
                width: self.image_width,
                height: self.image_height,
                depth_or_array_layers: 1,
            },
        );
        let frame = self.surface.get_current_texture()?;
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("terminal"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                occlusion_query_set: None,
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        self.queue.submit(Some(encoder.finish()));
        frame.present();
        Ok(())
    }
}

struct App {
    font_settings: FontSettings,
    cell: CellSize,
    window: Option<Arc<Window>>,
    gpu: Option<Gpu>,
    fonts: Vec<FontVec>,
    text_renderer: TextRenderer,
    pixels: Vec<u8>,
    terminal: Terminal,
    parser: Parser,
    pty: Option<Pty>,
    input: Option<Sender<Vec<u8>>>,
    proxy: EventLoopProxy<UserEvent>,
    output: Arc<OutputBuffer>,
    modifiers: ModifiersState,
    ended: bool,
    drawable: bool,
    preedit: String,
    scroll_offset: usize,
    selection: Option<(usize, usize)>,
    dragging: bool,
    mouse_cell: usize,
    mouse_button: Option<u8>,
    clipboard: Option<arboard::Clipboard>,
    last_frame: Option<Instant>,
    output_redraw_pending: bool,
    input_redraw_pending: bool,
}

impl App {
    fn new(proxy: EventLoopProxy<UserEvent>) -> Self {
        let settings = FontSettings::load();
        let fonts = load_fonts(&settings.name);
        let cell = fonts.first().map_or(
            CellSize {
                width: 10,
                height: 20,
            },
            |font| CellSize::for_font(font, settings.size),
        );
        Self {
            font_settings: settings.clone(),
            cell,
            window: None,
            gpu: None,
            fonts,
            text_renderer: TextRenderer::new(&settings, cell),
            pixels: Vec::new(),
            terminal: Terminal::new(ROWS, COLS),
            parser: Parser::default(),
            pty: None,
            input: None,
            proxy,
            output: Arc::new(OutputBuffer::default()),
            modifiers: ModifiersState::empty(),
            ended: false,
            drawable: true,
            preedit: String::new(),
            scroll_offset: 0,
            selection: None,
            dragging: false,
            mouse_cell: 0,
            mouse_button: None,
            clipboard: None,
            last_frame: None,
            output_redraw_pending: false,
            input_redraw_pending: false,
        }
    }

    fn message(&mut self, message: &str) {
        self.parser
            .advance(format!("\r\n{message}\r\n").as_bytes(), |action| {
                self.terminal.process(action)
            });
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn close(&mut self, event_loop: &ActiveEventLoop) {
        self.input.take();
        if let Some(pty) = &mut self.pty {
            let _ = pty.terminate();
        }
        // EGL must terminate while winit's Wayland connection still exists.
        self.gpu.take();
        self.window.take();
        event_loop.exit();
    }

    fn start_shell(&mut self) {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
        let rows = self.terminal.screen.rows();
        let cols = self.terminal.screen.cols();
        let options = PtyOptions {
            program: shell,
            args: vec![],
            cwd: None,
            env: HashMap::from([("TERM".into(), "xterm-256color".into())]),
            size: PtySize {
                rows: rows as u16,
                cols: cols as u16,
                pixel_width: (cols * self.cell.width) as u16,
                pixel_height: (rows * self.cell.height) as u16,
            },
        };
        match Pty::spawn(options) {
            Ok(pty) => match pty.clone_reader() {
                Ok(mut reader) => {
                    let proxy = self.proxy.clone();
                    let output = self.output.clone();
                    thread::spawn(move || {
                        let mut buffer = [0; 65536];
                        loop {
                            match reader.read(&mut buffer) {
                                Ok(0) => break,
                                Ok(n) => {
                                    if output.push(&buffer[..n])
                                        && proxy.send_event(UserEvent::Output).is_err()
                                    {
                                        return;
                                    }
                                }
                                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {
                                    continue;
                                }
                                Err(error) if error.raw_os_error() == Some(5) => break,
                                Err(error) => {
                                    let _ = proxy.send_event(UserEvent::End(format!(
                                        "PTY read error: {error}"
                                    )));
                                    return;
                                }
                            }
                        }
                        let _ = proxy.send_event(UserEvent::End(String::new()));
                    });
                    match pty.clone_reader() {
                        Ok(mut writer) => {
                            let (sender, receiver) = mpsc::channel::<Vec<u8>>();
                            let proxy = self.proxy.clone();
                            thread::spawn(move || {
                                for bytes in receiver {
                                    if let Err(error) = writer.write_all(&bytes) {
                                        let _ = proxy.send_event(UserEvent::WriteError(format!(
                                            "PTY write error: {error}"
                                        )));
                                        break;
                                    }
                                }
                            });
                            self.input = Some(sender);
                            self.pty = Some(pty);
                        }
                        Err(error) => self.message(&format!("Cannot write PTY: {error}")),
                    }
                }
                Err(error) => self.message(&format!("Cannot read PTY: {error}")),
            },
            Err(error) => self.message(&format!("Cannot start shell: {error}")),
        }
    }

    fn send_key(&mut self, key: &Key) {
        let bytes: Vec<u8> = match key {
            Key::Named(NamedKey::Enter) => vec![b'\r'],
            Key::Named(NamedKey::Backspace) => vec![0x7f],
            Key::Named(NamedKey::Tab) => vec![b'\t'],
            Key::Named(NamedKey::Space) => vec![b' '],
            Key::Named(NamedKey::Escape) => vec![0x1b],
            Key::Named(NamedKey::ArrowUp) => self.cursor_key(b'A'),
            Key::Named(NamedKey::ArrowDown) => self.cursor_key(b'B'),
            Key::Named(NamedKey::ArrowRight) => self.cursor_key(b'C'),
            Key::Named(NamedKey::ArrowLeft) => self.cursor_key(b'D'),
            Key::Named(NamedKey::Delete) => b"\x1b[3~".to_vec(),
            Key::Named(NamedKey::Home) => self.cursor_key(b'H'),
            Key::Named(NamedKey::End) => self.cursor_key(b'F'),
            Key::Character(text) if self.modifiers.control_key() => text
                .bytes()
                .next()
                .and_then(control_byte)
                .into_iter()
                .collect(),
            Key::Character(text) if !self.modifiers.alt_key() && !self.modifiers.super_key() => {
                text.as_bytes().to_vec()
            }
            _ => Vec::new(),
        };
        if !bytes.is_empty() {
            self.send_input(bytes);
        }
    }

    fn cursor_key(&self, suffix: u8) -> Vec<u8> {
        vec![
            0x1b,
            if self.terminal.application_cursor {
                b'O'
            } else {
                b'['
            },
            suffix,
        ]
    }

    fn send_mouse(&mut self, button: u8, release: bool) {
        if self.terminal.mouse_mode == MouseMode::Off {
            return;
        }
        let cols = self.terminal.screen.cols();
        if cols == 0 {
            return;
        }
        let x = self.mouse_cell % cols + 1;
        let y = self.mouse_cell / cols + 1;
        let mut code = button;
        if self.modifiers.shift_key() {
            code += 4;
        }
        if self.modifiers.alt_key() {
            code += 8;
        }
        if self.modifiers.control_key() {
            code += 16;
        }
        if self.terminal.sgr_mouse {
            self.send_input(
                format!("\x1b[<{code};{x};{y}{}", if release { 'm' } else { 'M' }).into_bytes(),
            );
        } else if x <= 223 && y <= 223 {
            self.send_input(vec![
                0x1b,
                b'[',
                b'M',
                32 + if release { 3 } else { code },
                (32 + x) as u8,
                (32 + y) as u8,
            ]);
        }
    }

    fn send_input(&mut self, bytes: Vec<u8>) {
        if let Some(input) = &self.input {
            if input.send(bytes).is_err() {
                self.message("PTY writer stopped");
            } else {
                self.input_redraw_pending = true;
            }
        }
    }

    fn resize_grid(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        let cols =
            ((width as usize / self.cell.width).max(1)).min(u16::MAX as usize / self.cell.width);
        let rows =
            ((height as usize / self.cell.height).max(1)).min(u16::MAX as usize / self.cell.height);
        if cols == self.terminal.screen.cols() && rows == self.terminal.screen.rows() {
            return;
        }
        self.terminal.resize(rows, cols);
        if let Some(gpu) = &mut self.gpu {
            gpu.resize_grid(cols, rows);
        }
        if let Some(pty) = &self.pty
            && let Err(error) = pty.resize(PtySize {
                rows: rows as u16,
                cols: cols as u16,
                pixel_width: (cols * self.cell.width) as u16,
                pixel_height: (rows * self.cell.height) as u16,
            })
        {
            self.message(&format!("PTY resize error: {error}"));
        }
        self.selection = None;
        self.scroll_offset = 0;
    }

    fn scale_text(&mut self, scale_factor: f64) {
        let mut settings = self.font_settings.clone();
        settings.size *= scale_factor as f32;
        if let Some(font) = self.fonts.first() {
            self.cell = CellSize::for_font(font, settings.size);
        }
        self.text_renderer = TextRenderer::new(&settings, self.cell);
        if let Some(gpu) = &mut self.gpu {
            gpu.cell = self.cell;
            gpu.resize_grid(self.terminal.screen.cols(), self.terminal.screen.rows());
        }
    }

    fn update_ime_area(&self) {
        if let Some(window) = &self.window {
            let cursor = self.terminal.cursor.position;
            window.set_ime_cursor_area(
                winit::dpi::PhysicalPosition::new(
                    (cursor.col * self.cell.width) as i32,
                    (cursor.row * self.cell.height) as i32,
                ),
                winit::dpi::PhysicalSize::new(self.cell.width as u32, self.cell.height as u32),
            );
        }
    }

    fn copy_selection(&mut self) {
        let Some((a, b)) = self.selection else {
            return;
        };
        let cols = self.terminal.screen.cols();
        let cells = self.terminal.screen.view_cells(self.scroll_offset);
        let text = selected_text(&cells, cols, a, b);
        if self.clipboard.is_none() {
            self.clipboard = arboard::Clipboard::new().ok();
        }
        if let Some(clipboard) = &mut self.clipboard {
            if let Err(error) = clipboard.set_text(text) {
                eprintln!("Clipboard copy error: {error}");
            }
        } else {
            eprintln!("Clipboard is unavailable");
        }
    }

    fn paste(&mut self) {
        if self.clipboard.is_none() {
            self.clipboard = arboard::Clipboard::new().ok();
        }
        let Some(clipboard) = &mut self.clipboard else {
            eprintln!("Clipboard is unavailable");
            return;
        };
        let Ok(text) = clipboard.get_text() else {
            eprintln!("Clipboard does not contain text");
            return;
        };
        if self.terminal.bracketed_paste {
            let mut bytes = b"\x1b[200~".to_vec();
            bytes.extend_from_slice(text.as_bytes());
            bytes.extend_from_slice(b"\x1b[201~");
            self.send_input(bytes);
        } else {
            self.send_input(text.into_bytes());
        }
    }
}

fn selected_text(cells: &[ruterm_core::screen::Cell], cols: usize, a: usize, b: usize) -> String {
    let (start, end) = (a.min(b), a.max(b));
    let mut text = String::new();
    for row in start / cols..=end / cols {
        if row > start / cols {
            text.push('\n');
        }
        let left = if row == start / cols { start % cols } else { 0 };
        let right = if row == end / cols {
            end % cols + 1
        } else {
            cols
        };
        let mut line = String::new();
        for cell in &cells[row * cols + left..row * cols + right] {
            if !cell.wide_continuation {
                line.push(cell.ch);
                line.push_str(&cell.combining);
            }
        }
        text.push_str(line.trim_end());
    }
    text
}

fn control_byte(byte: u8) -> Option<u8> {
    match byte.to_ascii_lowercase() {
        b'a'..=b'z' => Some(byte.to_ascii_lowercase() - b'a' + 1),
        b'[' => Some(27),
        b'\\' => Some(28),
        b']' => Some(29),
        b'^' => Some(30),
        b'_' => Some(31),
        b' ' | b'@' => Some(0),
        _ => None,
    }
}

fn should_forward_key(key: &Key, modifiers: ModifiersState, preedit: &str) -> bool {
    preedit.is_empty() || !matches!(key, Key::Character(_)) || modifiers.control_key()
}

impl ApplicationHandler<UserEvent> for App {
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if !self.output_redraw_pending {
            event_loop.set_control_flow(winit::event_loop::ControlFlow::Wait);
            return;
        }
        let deadline = self
            .last_frame
            .map_or_else(Instant::now, |time| time + FRAME_INTERVAL);
        if Instant::now() >= deadline {
            self.output_redraw_pending = false;
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        } else {
            event_loop.set_control_flow(winit::event_loop::ControlFlow::WaitUntil(deadline));
        }
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        self.input.take();
        if let Some(pty) = &mut self.pty {
            let _ = pty.terminate();
        }
        self.gpu.take();
        self.window.take();
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = WindowAttributes::default()
            .with_title("ruterm")
            .with_inner_size(LogicalSize::new(
                (COLS * self.cell.width) as u32,
                (ROWS * self.cell.height) as u32,
            ));
        match event_loop.create_window(attrs) {
            Ok(window) => {
                let window = Arc::new(window);
                self.scale_text(window.scale_factor());
                match Gpu::new(window.clone(), self.cell) {
                    Ok(gpu) => self.gpu = Some(gpu),
                    Err(error) => {
                        eprintln!("GPU initialization failed: {error}");
                        event_loop.exit();
                        return;
                    }
                }
                self.window = Some(window);
                self.window.as_ref().unwrap().set_ime_allowed(true);
                let size = self.window.as_ref().unwrap().inner_size();
                self.resize_grid(size.width, size.height);
                self.start_shell();
                self.window.as_ref().unwrap().request_redraw();
            }
            Err(error) => {
                eprintln!("Window creation failed: {error}");
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if self.window.as_ref().is_none_or(|window| window.id() != id) {
            return;
        }
        match event {
            WindowEvent::CloseRequested => {
                self.close(event_loop);
            }
            WindowEvent::Resized(size) => {
                self.drawable = size.width > 0 && size.height > 0;
                if self.drawable {
                    if let Some(gpu) = &mut self.gpu {
                        gpu.resize(size.width, size.height);
                    }
                    self.resize_grid(size.width, size.height);
                    self.window.as_ref().unwrap().request_redraw();
                }
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.scale_text(scale_factor);
                let size = self.window.as_ref().unwrap().inner_size();
                self.resize_grid(size.width, size.height);
                self.window.as_ref().unwrap().request_redraw();
            }
            WindowEvent::ModifiersChanged(modifiers) => self.modifiers = modifiers.state(),
            WindowEvent::CursorMoved { position, .. } => {
                let cols = self.terminal.screen.cols();
                let rows = self.terminal.screen.rows();
                let col = (position.x.max(0.0) as usize / self.cell.width).min(cols - 1);
                let row = (position.y.max(0.0) as usize / self.cell.height).min(rows - 1);
                let old_cell = self.mouse_cell;
                self.mouse_cell = row * cols + col;
                if !self.modifiers.shift_key()
                    && old_cell != self.mouse_cell
                    && (self.terminal.mouse_mode == MouseMode::Motion
                        || (self.terminal.mouse_mode == MouseMode::Drag
                            && self.mouse_button.is_some()))
                {
                    self.send_mouse(self.mouse_button.unwrap_or(3) + 32, false);
                }
                if self.dragging {
                    if let Some((start, _)) = self.selection {
                        self.selection = Some((start, self.mouse_cell));
                    }
                    self.window.as_ref().unwrap().request_redraw();
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let code = match button {
                    MouseButton::Left => Some(0),
                    MouseButton::Middle => Some(1),
                    MouseButton::Right => Some(2),
                    _ => None,
                };
                if let Some(code) = code {
                    let pressed = state == ElementState::Pressed;
                    if self.terminal.mouse_mode != MouseMode::Off && !self.modifiers.shift_key() {
                        self.send_mouse(code, !pressed);
                        self.mouse_button = if pressed { Some(code) } else { None };
                    } else if code == 0 {
                        self.dragging = pressed;
                        if pressed {
                            self.selection = Some((self.mouse_cell, self.mouse_cell));
                        }
                    }
                    if !pressed {
                        self.mouse_button = None;
                    }
                }
                self.window.as_ref().unwrap().request_redraw();
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let lines = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y.round() as isize * 3,
                    MouseScrollDelta::PixelDelta(position) => {
                        (position.y / self.cell.height as f64).round() as isize
                    }
                };
                if self.terminal.mouse_mode != MouseMode::Off && !self.modifiers.shift_key() {
                    for _ in 0..lines.unsigned_abs().min(20) {
                        self.send_mouse(if lines > 0 { 64 } else { 65 }, false);
                    }
                    return;
                }
                self.scroll_offset = self
                    .scroll_offset
                    .saturating_add_signed(lines)
                    .min(self.terminal.screen.scrollback.len());
                self.selection = None;
                self.window.as_ref().unwrap().request_redraw();
            }
            WindowEvent::Ime(ime) => {
                match ime {
                    Ime::Enabled => {}
                    Ime::Disabled => {
                        self.preedit.clear();
                    }
                    Ime::Preedit(text, _) => self.preedit = text,
                    Ime::Commit(text) => {
                        self.preedit.clear();
                        self.send_input(text.into_bytes());
                    }
                }
                self.window.as_ref().unwrap().request_redraw();
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                if self.terminal.application_keypad {
                    let suffix = match event.physical_key {
                        PhysicalKey::Code(KeyCode::Numpad0) => Some(b'p'),
                        PhysicalKey::Code(KeyCode::Numpad1) => Some(b'q'),
                        PhysicalKey::Code(KeyCode::Numpad2) => Some(b'r'),
                        PhysicalKey::Code(KeyCode::Numpad3) => Some(b's'),
                        PhysicalKey::Code(KeyCode::Numpad4) => Some(b't'),
                        PhysicalKey::Code(KeyCode::Numpad5) => Some(b'u'),
                        PhysicalKey::Code(KeyCode::Numpad6) => Some(b'v'),
                        PhysicalKey::Code(KeyCode::Numpad7) => Some(b'w'),
                        PhysicalKey::Code(KeyCode::Numpad8) => Some(b'x'),
                        PhysicalKey::Code(KeyCode::Numpad9) => Some(b'y'),
                        PhysicalKey::Code(KeyCode::NumpadEnter) => Some(b'M'),
                        PhysicalKey::Code(KeyCode::NumpadAdd) => Some(b'k'),
                        PhysicalKey::Code(KeyCode::NumpadSubtract) => Some(b'm'),
                        PhysicalKey::Code(KeyCode::NumpadMultiply) => Some(b'j'),
                        PhysicalKey::Code(KeyCode::NumpadDivide) => Some(b'o'),
                        PhysicalKey::Code(KeyCode::NumpadDecimal) => Some(b'n'),
                        _ => None,
                    };
                    if let Some(suffix) = suffix {
                        self.send_input(vec![0x1b, b'O', suffix]);
                        return;
                    }
                }
                if self.modifiers.control_key()
                    && self.modifiers.shift_key()
                    && event.physical_key == PhysicalKey::Code(KeyCode::KeyC)
                {
                    self.copy_selection();
                } else if self.modifiers.control_key()
                    && self.modifiers.shift_key()
                    && event.physical_key == PhysicalKey::Code(KeyCode::KeyV)
                {
                    self.paste();
                } else if should_forward_key(&event.logical_key, self.modifiers, &self.preedit) {
                    self.send_key(&event.logical_key);
                }
            }
            WindowEvent::RedrawRequested if self.drawable => {
                self.last_frame = Some(Instant::now());
                self.update_ime_area();
                rasterize(
                    &self.terminal,
                    self.cell,
                    &mut self.text_renderer,
                    &self.preedit,
                    self.scroll_offset,
                    self.selection,
                    &mut self.pixels,
                );
                if let Some(gpu) = &mut self.gpu {
                    match gpu.render(&self.pixels) {
                        Ok(()) => {}
                        Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                            let size = self.window.as_ref().unwrap().inner_size();
                            gpu.resize(size.width, size.height);
                        }
                        Err(wgpu::SurfaceError::OutOfMemory) => event_loop.exit(),
                        Err(error) => eprintln!("Surface error: {error}"),
                    }
                }
            }
            _ => {}
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Output => {
                let bytes = self.output.take();
                let old_history = self.terminal.screen.scrollback.len();
                let was_alternate = self.terminal.is_alternate_screen();
                self.parser
                    .advance(&bytes, |action| self.terminal.process(action));
                for response in self.terminal.take_responses() {
                    self.send_input(response);
                }
                if let Some(window) = &self.window {
                    window.set_title(&self.terminal.title);
                }
                if was_alternate != self.terminal.is_alternate_screen() {
                    self.scroll_offset = 0;
                    self.selection = None;
                }
                if self.scroll_offset > 0 {
                    self.scroll_offset = self
                        .scroll_offset
                        .saturating_add(
                            self.terminal
                                .screen
                                .scrollback
                                .len()
                                .saturating_sub(old_history),
                        )
                        .min(self.terminal.screen.scrollback.len());
                }
                self.update_ime_area();
                if self.input_redraw_pending {
                    self.input_redraw_pending = false;
                    self.output_redraw_pending = false;
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                } else {
                    self.output_redraw_pending = true;
                }
            }
            UserEvent::End(error) if !self.ended => {
                self.ended = true;
                if error.is_empty() {
                    self.close(event_loop);
                } else {
                    self.message(&error);
                }
            }
            UserEvent::WriteError(error) => self.message(&error),
            _ => {}
        }
    }
}

fn load_fonts(name: &str) -> Vec<FontVec> {
    [name, "sans:lang=ja"]
        .iter()
        .filter_map(|pattern| {
            let output = Command::new("fc-match")
                .args(["-f", "%{file}", pattern])
                .output()
                .ok()?;
            let path = String::from_utf8(output.stdout).ok()?;
            FontVec::try_from_vec(std::fs::read(path).ok()?).ok()
        })
        .collect()
}

#[derive(Clone, Hash, PartialEq, Eq)]
struct TextKey {
    text: String,
    span: usize,
    bold: bool,
    italic: bool,
}

struct CellGlyph {
    width: usize,
    height: usize,
    rgba: Vec<u8>,
}

struct TextRenderer {
    font_system: FontSystem,
    swash_cache: SwashCache,
    glyphs: HashMap<TextKey, CellGlyph>,
    family: String,
    font_size: f32,
    cell: CellSize,
}

impl TextRenderer {
    fn new(settings: &FontSettings, cell: CellSize) -> Self {
        let family = Command::new("fc-match")
            .args(["-f", "%{family}", &settings.name])
            .output()
            .ok()
            .and_then(|output| String::from_utf8(output.stdout).ok())
            .and_then(|names| names.split(',').next().map(str::to_owned))
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| settings.name.clone());
        Self {
            font_system: FontSystem::new(),
            swash_cache: SwashCache::new(),
            glyphs: HashMap::new(),
            family,
            font_size: settings.size,
            cell,
        }
    }

    fn glyph(&mut self, text: &str, span: usize, flags: Flags) -> &CellGlyph {
        let key = TextKey {
            text: text.to_owned(),
            span,
            bold: flags.contains(Flags::BOLD),
            italic: flags.contains(Flags::ITALIC),
        };
        if !self.glyphs.contains_key(&key) {
            if self.glyphs.len() >= 4096 {
                self.glyphs.clear();
            }
            let glyph = self.shape_glyph(&key);
            self.glyphs.insert(key.clone(), glyph);
        }
        &self.glyphs[&key]
    }

    fn shape_glyph(&mut self, key: &TextKey) -> CellGlyph {
        let cell_width = self.cell.width * key.span;
        let height = self.cell.height;
        let mut buffer = Buffer::new_empty(Metrics::new(self.font_size, height as f32));
        buffer.set_wrap(Wrap::None);
        let attrs = Attrs::new()
            .family(Family::Name(&self.family))
            .weight(if key.bold {
                Weight::BOLD
            } else {
                Weight::NORMAL
            })
            .style(if key.italic {
                Style::Italic
            } else {
                Style::Normal
            });
        buffer.set_text(&key.text, &attrs, Shaping::Advanced, None);
        buffer.shape_until_scroll(&mut self.font_system, false);
        let advance = buffer
            .layout_runs()
            .flat_map(|run| run.glyphs.iter())
            .map(|glyph| glyph.x + glyph.w)
            .fold(0.0_f32, f32::max);
        let x_offset = ((cell_width as f32 - advance).max(0.0) / 2.0).round() as i32;
        let mut samples = Vec::new();
        buffer.draw(
            &mut self.font_system,
            &mut self.swash_cache,
            TextColor::rgb(255, 255, 255),
            |x, y, _w, _h, color| {
                let x = x + x_offset;
                let rgba = color.as_rgba();
                if x < 0 || y < 0 || y >= height as i32 || rgba[3] == 0 {
                    return;
                }
                samples.push((x as usize, y as usize, rgba));
            },
        );
        let width = samples
            .iter()
            .map(|(x, _, _)| x + 1)
            .max()
            .unwrap_or(0)
            .max(cell_width);
        let mut rgba = vec![0; width * height * 4];
        for (x, y, [r, g, b, a]) in samples {
            let offset = (y * width + x) * 4;
            let old_a = rgba[offset + 3] as u16;
            let new_a = a as u16 + old_a * (255 - a as u16) / 255;
            if new_a == 0 {
                continue;
            }
            for (channel, source) in [r, g, b].into_iter().enumerate() {
                let old = rgba[offset + channel] as u16;
                rgba[offset + channel] = ((source as u16 * a as u16
                    + old * old_a * (255 - a as u16) / 255)
                    / new_a) as u8;
            }
            rgba[offset + 3] = new_a as u8;
        }
        CellGlyph {
            width,
            height,
            rgba,
        }
    }
}

fn rasterize(
    terminal: &Terminal,
    cell_size: CellSize,
    text_renderer: &mut TextRenderer,
    preedit: &str,
    scroll_offset: usize,
    selection: Option<(usize, usize)>,
    pixels: &mut Vec<u8>,
) {
    let cols = terminal.screen.cols();
    let rows = terminal.screen.rows();
    let width = cols * cell_size.width;
    pixels.resize(width * rows * cell_size.height * 4, 0);
    let mut cells: Cow<'_, [ruterm_core::screen::Cell]> = if scroll_offset == 0 {
        Cow::Borrowed(&terminal.screen.cells)
    } else {
        Cow::Owned(terminal.screen.view_cells(scroll_offset))
    };
    if scroll_offset == 0 && !preedit.is_empty() {
        let row = terminal.cursor.position.row;
        for (col, ch) in (terminal.cursor.position.col..).zip(preedit.chars()) {
            if row >= rows || col >= cols {
                break;
            }
            let index = row * cols + col;
            cells.to_mut()[index] = ruterm_core::screen::Cell {
                ch,
                attrs: ruterm_core::attributes::Attributes {
                    underline: ruterm_core::attributes::UnderlineStyle::Single,
                    ..Default::default()
                },
                ..Default::default()
            };
        }
    }
    for row in 0..rows {
        for col in 0..cols {
            let cell = &cells[row * cols + col];
            let mut fg = color(cell.attrs.fg, [220, 220, 220]);
            let mut bg = color(cell.attrs.bg, [20, 22, 27]);
            if cell.attrs.flags.contains(Flags::INVERSE) {
                std::mem::swap(&mut fg, &mut bg);
            }
            if scroll_offset == 0
                && preedit.is_empty()
                && terminal.cursor.visible
                && terminal.cursor_shape == CursorShape::Block
                && terminal.cursor.position.row == row
                && terminal.cursor.position.col == col
            {
                std::mem::swap(&mut fg, &mut bg);
            }
            if selection.is_some_and(|(a, b)| {
                let index = row * cols + col;
                index >= a.min(b) && index <= a.max(b)
            }) {
                std::mem::swap(&mut fg, &mut bg);
            }
            for y in 0..cell_size.height {
                for x in 0..cell_size.width {
                    put_pixel(
                        pixels,
                        width,
                        col * cell_size.width + x,
                        row * cell_size.height + y,
                        bg,
                    );
                }
            }
        }
    }
    for row in 0..rows {
        for col in 0..cols {
            let cell = &cells[row * cols + col];
            if cell.wide_continuation {
                continue;
            }
            let mut fg = color(cell.attrs.fg, [220, 220, 220]);
            let mut bg = color(cell.attrs.bg, [20, 22, 27]);
            if cell.attrs.flags.contains(Flags::INVERSE) {
                std::mem::swap(&mut fg, &mut bg);
            }
            if scroll_offset == 0
                && preedit.is_empty()
                && terminal.cursor.visible
                && terminal.cursor_shape == CursorShape::Block
                && terminal.cursor.position.row == row
                && terminal.cursor.position.col == col
            {
                std::mem::swap(&mut fg, &mut bg);
            }
            if selection.is_some_and(|(a, b)| {
                let index = row * cols + col;
                index >= a.min(b) && index <= a.max(b)
            }) {
                std::mem::swap(&mut fg, &mut bg);
            }
            let span = if col + 1 < cols && cells[row * cols + col + 1].wide_continuation {
                2
            } else {
                1
            };
            if !cell.attrs.flags.contains(Flags::HIDDEN)
                && (cell.ch != ' ' || !cell.combining.is_empty())
            {
                let mut text = cell.ch.to_string();
                text.push_str(&cell.combining);
                let glyph = text_renderer.glyph(&text, span, cell.attrs.flags);
                // Box drawing glyphs must not paint into the following cell: a
                // font's antialiased right edge can otherwise be blended with
                // the next glyph and make a join look heavier.
                let glyph_width =
                    if cell.combining.is_empty() && ('\u{2500}'..='\u{257f}').contains(&cell.ch) {
                        glyph.width.min(span * cell_size.width)
                    } else {
                        glyph.width
                    };
                for y in 0..glyph.height {
                    for x in 0..glyph_width.min(width - col * cell_size.width) {
                        let source = (y * glyph.width + x) * 4;
                        let alpha = glyph.rgba[source + 3] as u16;
                        if alpha == 0 {
                            continue;
                        }
                        let target =
                            ((row * cell_size.height + y) * width + col * cell_size.width + x) * 4;
                        let color_glyph = glyph.rgba[source..source + 3] != [255, 255, 255];
                        for channel in 0..3 {
                            let ink = if color_glyph {
                                glyph.rgba[source + channel]
                            } else {
                                fg[channel]
                            };
                            pixels[target + channel] = ((ink as u16 * alpha
                                + pixels[target + channel] as u16 * (255 - alpha))
                                / 255) as u8;
                        }
                    }
                }
            }
            if cell.attrs.underline != ruterm_core::attributes::UnderlineStyle::None {
                use ruterm_core::attributes::UnderlineStyle;
                let underline_fg = color(cell.attrs.underline_color, fg);
                for x in 0..span * cell_size.width {
                    let style = cell.attrs.underline;
                    if (style == UnderlineStyle::Dotted && x % 3 != 0)
                        || (style == UnderlineStyle::Dashed && x % 6 >= 4)
                    {
                        continue;
                    }
                    let baseline = ((row + 1) * cell_size.height).saturating_sub(2);
                    let y = if style == UnderlineStyle::Curly {
                        baseline.saturating_sub((x / 2) % 2)
                    } else {
                        baseline
                    };
                    put_pixel(pixels, width, col * cell_size.width + x, y, underline_fg);
                    if style == UnderlineStyle::Double && cell_size.height >= 4 {
                        put_pixel(
                            pixels,
                            width,
                            col * cell_size.width + x,
                            baseline - 2,
                            underline_fg,
                        );
                    }
                }
            }
        }
    }
    if scroll_offset == 0
        && preedit.is_empty()
        && terminal.cursor.visible
        && terminal.cursor_shape != CursorShape::Block
    {
        let row = terminal.cursor.position.row;
        let col = terminal.cursor.position.col;
        if row < rows && col < cols {
            let ink = [220, 220, 220];
            match terminal.cursor_shape {
                CursorShape::Underline => {
                    for x in 0..cell_size.width {
                        put_pixel(
                            pixels,
                            width,
                            col * cell_size.width + x,
                            (row + 1) * cell_size.height - 2,
                            ink,
                        );
                    }
                }
                CursorShape::Bar => {
                    for y in 0..cell_size.height {
                        put_pixel(
                            pixels,
                            width,
                            col * cell_size.width,
                            row * cell_size.height + y,
                            ink,
                        );
                    }
                }
                CursorShape::Block => {}
            }
        }
    }
}

fn put_pixel(pixels: &mut [u8], width: usize, x: usize, y: usize, color: [u8; 3]) {
    let offset = (y * width + x) * 4;
    pixels[offset..offset + 4].copy_from_slice(&[color[0], color[1], color[2], 255]);
}

fn color(color: Color, default: [u8; 3]) -> [u8; 3] {
    const ANSI: [[u8; 3]; 16] = [
        [0, 0, 0],
        [205, 49, 49],
        [13, 188, 121],
        [229, 229, 16],
        [36, 114, 200],
        [188, 63, 188],
        [17, 168, 205],
        [229, 229, 229],
        [102, 102, 102],
        [241, 76, 76],
        [35, 209, 139],
        [245, 245, 67],
        [59, 142, 234],
        [214, 112, 214],
        [41, 184, 219],
        [255, 255, 255],
    ];
    match color {
        Color::Default => default,
        Color::Rgb(r, g, b) => [r, g, b],
        Color::Indexed(i) if i < 16 => ANSI[i as usize],
        Color::Indexed(i) if i < 232 => {
            let i = i - 16;
            let channel = |v| if v == 0 { 0 } else { 55 + 40 * v };
            [channel(i / 36), channel((i / 6) % 6), channel(i % 6)]
        }
        Color::Indexed(i) => {
            let shade = 8 + (i - 232) * 10;
            [shade; 3]
        }
    }
}

fn main() -> Result<(), winit::error::EventLoopError> {
    let event_loop = EventLoop::<UserEvent>::with_user_event().build()?;
    let mut app = App::new(event_loop.create_proxy());
    if app.fonts.is_empty() {
        eprintln!("No monospace font found. Install fontconfig and a monospace font.");
        return Ok(());
    }
    event_loop.run_app(&mut app)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ruterm_core::attributes::{Attributes, Color};
    use ruterm_core::screen::Cell;

    #[test]
    fn output_buffer_coalesces_reads_without_losing_bytes() {
        let output = OutputBuffer::default();
        assert!(output.push(b"first"));
        assert!(!output.push(b" second"));
        assert_eq!(output.take(), b"first second");
        assert!(output.push(b"third"));
        assert_eq!(output.take(), b"third");
    }

    #[test]
    #[ignore]
    fn profile_full_screen_rasterization() {
        let Some(font) = load_fonts("monospace").into_iter().next() else {
            return;
        };
        let cell = CellSize::for_font(&font, DEFAULT_FONT_SIZE);
        let mut terminal = Terminal::new(ROWS, COLS);
        terminal.cursor.visible = false;
        let mut renderer = TextRenderer::new(&FontSettings::default(), cell);
        let mut pixels = Vec::new();
        let start = Instant::now();
        for _ in 0..10 {
            rasterize(&terminal, cell, &mut renderer, "", 0, None, &mut pixels);
        }
        eprintln!("blank average {:?}", start.elapsed() / 10);
        for (i, item) in terminal.screen.cells.iter_mut().enumerate() {
            item.ch = (b'!' + (i % 90) as u8) as char;
        }
        let start = Instant::now();
        rasterize(&terminal, cell, &mut renderer, "", 0, None, &mut pixels);
        let cold = start.elapsed();
        let start = Instant::now();
        for _ in 0..10 {
            rasterize(&terminal, cell, &mut renderer, "", 0, None, &mut pixels);
        }
        eprintln!("cold {cold:?}, warm average {:?}", start.elapsed() / 10);
    }

    #[test]
    fn parses_font_settings() {
        let settings =
            FontSettings::from_toml("[font]\nname = 'Noto Sans Mono'\nsize = 22.5\n").unwrap();
        assert_eq!(settings.name, "Noto Sans Mono");
        assert_eq!(settings.size, 22.5);
        assert_eq!(
            FontSettings::from_toml("[font]\nsize = 18\n").unwrap().name,
            "monospace"
        );
        assert!(FontSettings::from_toml("[font]\nsize = 0\n").is_err());
        assert!(FontSettings::from_toml("[font]\nname = ''\n").is_err());
    }

    #[test]
    fn control_keys_produce_terminal_bytes() {
        assert_eq!(control_byte(b'c'), Some(3));
        assert_eq!(control_byte(b'Z'), Some(26));
        assert_eq!(control_byte(b'['), Some(27));
        assert_eq!(control_byte(b'1'), None);
        assert!(should_forward_key(
            &Key::Character("a".into()),
            ModifiersState::empty(),
            ""
        ));
        assert!(!should_forward_key(
            &Key::Character("a".into()),
            ModifiersState::empty(),
            "変換"
        ));
    }

    #[test]
    fn rasterizes_cell_colors_and_cursor() {
        let mut terminal = Terminal::new(ROWS, COLS);
        terminal.cursor.visible = false;
        terminal.screen.set_cell(
            0,
            0,
            Cell {
                ch: ' ',
                attrs: Attributes {
                    fg: Color::Default,
                    bg: Color::Rgb(4, 5, 6),
                    ..Default::default()
                },
                ..Default::default()
            },
        );
        let mut pixels = Vec::new();
        let cell = CellSize {
            width: 10,
            height: 20,
        };
        let mut renderer = TextRenderer::new(&FontSettings::default(), cell);
        rasterize(&terminal, cell, &mut renderer, "", 0, None, &mut pixels);
        assert_eq!(&pixels[..4], &[4, 5, 6, 255]);
        terminal.cursor.visible = true;
        rasterize(&terminal, cell, &mut renderer, "", 0, None, &mut pixels);
        assert_eq!(&pixels[..4], &[220, 220, 220, 255]);
    }

    #[test]
    fn shapes_latin_cjk_and_combining_text() {
        let Some(font) = load_fonts("monospace").into_iter().next() else {
            return;
        };
        let cell = CellSize::for_font(&font, DEFAULT_FONT_SIZE);
        let mut renderer = TextRenderer::new(&FontSettings::default(), cell);
        for (text, span) in [("A", 1), ("日", 2), ("e\u{301}", 1)] {
            let glyph = renderer.glyph(text, span, Flags::empty());
            assert!(glyph.width >= cell.width * span);
            assert_eq!(glyph.height, cell.height);
            assert!(
                glyph.rgba.chunks_exact(4).any(|pixel| pixel[3] != 0),
                "{text}"
            );
        }
    }

    #[test]
    fn box_drawing_glyphs_do_not_overlap_adjacent_cells() {
        let cell = CellSize {
            width: 10,
            height: 20,
        };
        let mut renderer = TextRenderer::new(&FontSettings::default(), cell);
        let mut rgba = vec![0; 20 * cell.height * 4];
        rgba[(10 * 20 + 15) * 4..(10 * 20 + 15) * 4 + 4].copy_from_slice(&[255, 255, 255, 255]);
        renderer.glyphs.insert(
            TextKey {
                text: "─".into(),
                span: 1,
                bold: false,
                italic: false,
            },
            CellGlyph {
                width: 20,
                height: cell.height,
                rgba,
            },
        );
        let mut terminal = Terminal::new(1, 2);
        terminal.cursor.visible = false;
        terminal.screen.set_cell(
            0,
            0,
            Cell {
                ch: '─',
                ..Default::default()
            },
        );
        let mut pixels = Vec::new();
        rasterize(&terminal, cell, &mut renderer, "", 0, None, &mut pixels);
        let offset = (10 * 20 + 15) * 4;
        assert_eq!(&pixels[offset..offset + 3], &[20, 22, 27]);
    }

    #[test]
    fn rasterizes_overhanging_glyph_across_cell_boundary() {
        let cell = CellSize {
            width: 10,
            height: 20,
        };
        let mut renderer = TextRenderer::new(&FontSettings::default(), cell);
        let mut rgba = vec![0; 20 * cell.height * 4];
        rgba[(10 * 20 + 15) * 4..(10 * 20 + 15) * 4 + 4].copy_from_slice(&[255, 255, 255, 255]);
        renderer.glyphs.insert(
            TextKey {
                text: "\u{f013}".into(),
                span: 1,
                bold: false,
                italic: false,
            },
            CellGlyph {
                width: 20,
                height: cell.height,
                rgba,
            },
        );
        for cols in [1, 2] {
            let mut terminal = Terminal::new(1, cols);
            terminal.cursor.visible = false;
            terminal.screen.set_cell(
                0,
                0,
                Cell {
                    ch: '\u{f013}',
                    ..Default::default()
                },
            );
            let mut pixels = Vec::new();
            rasterize(&terminal, cell, &mut renderer, "", 0, None, &mut pixels);
            if cols == 2 {
                let offset = (10 * 20 + 15) * 4;
                assert_eq!(&pixels[offset..offset + 3], &[220, 220, 220]);
            }
        }
    }

    #[test]
    fn rasterizes_text_with_cell_foreground_color() {
        let Some(font) = load_fonts("monospace").into_iter().next() else {
            return;
        };
        let cell = CellSize::for_font(&font, DEFAULT_FONT_SIZE);
        let mut terminal = Terminal::new(1, 1);
        terminal.cursor.visible = false;
        terminal.screen.set_cell(
            0,
            0,
            Cell {
                ch: 'A',
                attrs: Attributes {
                    fg: Color::Rgb(255, 0, 0),
                    bg: Color::Rgb(0, 0, 0),
                    ..Default::default()
                },
                ..Default::default()
            },
        );
        let mut pixels = Vec::new();
        let mut renderer = TextRenderer::new(&FontSettings::default(), cell);
        rasterize(&terminal, cell, &mut renderer, "", 0, None, &mut pixels);
        assert!(
            pixels
                .chunks_exact(4)
                .any(|pixel| pixel[0] > 0 && pixel[1] == 0 && pixel[2] == 0)
        );
    }

    #[test]
    fn selected_text_preserves_line_breaks_and_combining_marks() {
        let mut terminal = Terminal::new(2, 3);
        let mut parser = Parser::default();
        parser.advance("a\u{301}b\r\nc".as_bytes(), |action| {
            terminal.process(action)
        });
        let cells = terminal.screen.view_cells(0);
        assert_eq!(selected_text(&cells, 3, 0, 3), "a\u{301}b\nc");
    }
}
