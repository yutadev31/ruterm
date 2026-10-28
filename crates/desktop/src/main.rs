use std::borrow::Cow;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::process::Command;
use std::sync::Arc;
use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::{Duration, Instant};

use ab_glyph::{Font, FontVec, PxScale, point};
use ruterm_core::attributes::{Color, Flags};
use ruterm_core::terminal::Terminal;
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
const CELL_W: usize = 10;
const CELL_H: usize = 20;
const WIDTH: u32 = (COLS * CELL_W) as u32;
const HEIGHT: u32 = (ROWS * CELL_H) as u32;
const FRAME_INTERVAL: Duration = Duration::from_millis(16);

enum UserEvent {
    Output(Vec<u8>, Sender<()>),
    WriteError(String),
    End(String),
}

struct Gpu {
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
    fn new(window: Arc<Window>) -> Result<Self, String> {
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
                width: WIDTH,
                height: HEIGHT,
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
            surface,
            device,
            queue,
            config,
            texture,
            layout,
            sampler,
            image_width: WIDTH,
            image_height: HEIGHT,
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
        self.image_width = (cols * CELL_W) as u32;
        self.image_height = (rows * CELL_H) as u32;
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
    window: Option<Arc<Window>>,
    gpu: Option<Gpu>,
    fonts: Vec<FontVec>,
    glyph_cache: HashMap<char, Option<GlyphBitmap>>,
    pixels: Vec<u8>,
    terminal: Terminal,
    parser: Parser,
    pty: Option<Pty>,
    input: Option<Sender<Vec<u8>>>,
    proxy: EventLoopProxy<UserEvent>,
    modifiers: ModifiersState,
    ended: bool,
    drawable: bool,
    preedit: String,
    scroll_offset: usize,
    selection: Option<(usize, usize)>,
    dragging: bool,
    mouse_cell: usize,
    clipboard: Option<arboard::Clipboard>,
    last_frame: Option<Instant>,
    output_redraw_pending: bool,
    input_redraw_pending: bool,
}

impl App {
    fn new(proxy: EventLoopProxy<UserEvent>) -> Self {
        Self {
            window: None,
            gpu: None,
            fonts: load_fonts(),
            glyph_cache: HashMap::new(),
            pixels: Vec::new(),
            terminal: Terminal::new(ROWS, COLS),
            parser: Parser::default(),
            pty: None,
            input: None,
            proxy,
            modifiers: ModifiersState::empty(),
            ended: false,
            drawable: true,
            preedit: String::new(),
            scroll_offset: 0,
            selection: None,
            dragging: false,
            mouse_cell: 0,
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
                pixel_width: (cols * CELL_W) as u16,
                pixel_height: (rows * CELL_H) as u16,
            },
        };
        match Pty::spawn(options) {
            Ok(pty) => match pty.clone_reader() {
                Ok(mut reader) => {
                    let proxy = self.proxy.clone();
                    thread::spawn(move || {
                        let mut buffer = [0; 8192];
                        let (ack, received) = mpsc::channel();
                        loop {
                            match reader.read(&mut buffer) {
                                Ok(0) => break,
                                Ok(n) => {
                                    if proxy
                                        .send_event(UserEvent::Output(
                                            buffer[..n].to_vec(),
                                            ack.clone(),
                                        ))
                                        .is_err()
                                        || received.recv().is_err()
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
            Key::Named(NamedKey::ArrowUp) => b"\x1b[A".to_vec(),
            Key::Named(NamedKey::ArrowDown) => b"\x1b[B".to_vec(),
            Key::Named(NamedKey::ArrowRight) => b"\x1b[C".to_vec(),
            Key::Named(NamedKey::ArrowLeft) => b"\x1b[D".to_vec(),
            Key::Named(NamedKey::Delete) => b"\x1b[3~".to_vec(),
            Key::Named(NamedKey::Home) => b"\x1b[H".to_vec(),
            Key::Named(NamedKey::End) => b"\x1b[F".to_vec(),
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
        let cols = ((width as usize / CELL_W).max(1)).min(u16::MAX as usize / CELL_W);
        let rows = ((height as usize / CELL_H).max(1)).min(u16::MAX as usize / CELL_H);
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
                pixel_width: (cols * CELL_W) as u16,
                pixel_height: (rows * CELL_H) as u16,
            })
        {
            self.message(&format!("PTY resize error: {error}"));
        }
        self.selection = None;
        self.scroll_offset = 0;
    }

    fn update_ime_area(&self) {
        if let Some(window) = &self.window {
            let cursor = self.terminal.cursor.position;
            window.set_ime_cursor_area(
                winit::dpi::PhysicalPosition::new(
                    (cursor.col * CELL_W) as i32,
                    (cursor.row * CELL_H) as i32,
                ),
                winit::dpi::PhysicalSize::new(CELL_W as u32, CELL_H as u32),
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
        self.send_input(text.into_bytes());
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
            .with_inner_size(LogicalSize::new(WIDTH, HEIGHT));
        match event_loop.create_window(attrs) {
            Ok(window) => {
                let window = Arc::new(window);
                match Gpu::new(window.clone()) {
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
            WindowEvent::ModifiersChanged(modifiers) => self.modifiers = modifiers.state(),
            WindowEvent::CursorMoved { position, .. } => {
                let cols = self.terminal.screen.cols();
                let rows = self.terminal.screen.rows();
                let col = (position.x.max(0.0) as usize / CELL_W).min(cols - 1);
                let row = (position.y.max(0.0) as usize / CELL_H).min(rows - 1);
                self.mouse_cell = row * cols + col;
                if self.dragging {
                    if let Some((start, _)) = self.selection {
                        self.selection = Some((start, self.mouse_cell));
                    }
                    self.window.as_ref().unwrap().request_redraw();
                }
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => {
                self.dragging = state == ElementState::Pressed;
                if self.dragging {
                    self.selection = Some((self.mouse_cell, self.mouse_cell));
                }
                self.window.as_ref().unwrap().request_redraw();
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let lines = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y.round() as isize * 3,
                    MouseScrollDelta::PixelDelta(position) => {
                        (position.y / CELL_H as f64).round() as isize
                    }
                };
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
                    &self.fonts,
                    &mut self.glyph_cache,
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
            UserEvent::Output(bytes, ack) => {
                let old_history = self.terminal.screen.scrollback.len();
                self.parser
                    .advance(&bytes, |action| self.terminal.process(action));
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
                let _ = ack.send(());
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

fn load_fonts() -> Vec<FontVec> {
    ["monospace", "sans:lang=ja"]
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

struct GlyphBitmap {
    x: i32,
    y: i32,
    width: usize,
    height: usize,
    alpha: Vec<u8>,
}

fn glyph_bitmap(fonts: &[FontVec], ch: char) -> Option<GlyphBitmap> {
    let font = fonts.iter().find(|font| font.glyph_id(ch).0 != 0)?;
    let glyph = font
        .glyph_id(ch)
        .with_scale_and_position(PxScale::from(17.0), point(1.0, 16.0));
    let outline = font.outline_glyph(glyph)?;
    let bounds = outline.px_bounds();
    let width = bounds.width() as usize;
    let height = bounds.height() as usize;
    let mut alpha = vec![0; width * height];
    outline
        .draw(|x, y, coverage| alpha[y as usize * width + x as usize] = (coverage * 255.0) as u8);
    Some(GlyphBitmap {
        x: bounds.min.x as i32,
        y: bounds.min.y as i32,
        width,
        height,
        alpha,
    })
}

fn rasterize(
    terminal: &Terminal,
    fonts: &[FontVec],
    glyph_cache: &mut HashMap<char, Option<GlyphBitmap>>,
    preedit: &str,
    scroll_offset: usize,
    selection: Option<(usize, usize)>,
    pixels: &mut Vec<u8>,
) {
    let cols = terminal.screen.cols();
    let rows = terminal.screen.rows();
    let width = cols * CELL_W;
    pixels.resize(width * rows * CELL_H * 4, 0);
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
            for y in 0..CELL_H {
                for x in 0..CELL_W {
                    put_pixel(pixels, width, col * CELL_W + x, row * CELL_H + y, bg);
                }
            }
        }
    }
    for row in 0..rows {
        for col in 0..cols {
            let cell = &cells[row * cols + col];
            if cell.wide_continuation || (cell.ch == ' ' && cell.combining.is_empty()) {
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
            for ch in std::iter::once(cell.ch).chain(cell.combining.chars()) {
                let Some(bitmap) = glyph_cache
                    .entry(ch)
                    .or_insert_with(|| glyph_bitmap(fonts, ch))
                else {
                    continue;
                };
                for y in 0..bitmap.height {
                    for x in 0..bitmap.width {
                        let alpha = bitmap.alpha[y * bitmap.width + x] as f32 / 255.0;
                        if alpha == 0.0 {
                            continue;
                        }
                        let px = (col * CELL_W) as i32 + bitmap.x + x as i32;
                        let py = (row * CELL_H) as i32 + bitmap.y + y as i32;
                        if px >= (col * CELL_W) as i32
                            && px < ((col + span) * CELL_W) as i32
                            && py >= (row * CELL_H) as i32
                            && py < ((row + 1) * CELL_H) as i32
                        {
                            let mixed = std::array::from_fn(|i| {
                                (fg[i] as f32 * alpha + bg[i] as f32 * (1.0 - alpha)) as u8
                            });
                            put_pixel(pixels, width, px as usize, py as usize, mixed);
                        }
                    }
                }
            }
            if cell.attrs.underline != ruterm_core::attributes::UnderlineStyle::None {
                for x in 0..span * CELL_W {
                    put_pixel(
                        pixels,
                        width,
                        col * CELL_W + x,
                        row * CELL_H + CELL_H - 2,
                        fg,
                    );
                }
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
        rasterize(
            &terminal,
            &[],
            &mut HashMap::new(),
            "",
            0,
            None,
            &mut pixels,
        );
        assert_eq!(&pixels[..4], &[4, 5, 6, 255]);
        terminal.cursor.visible = true;
        rasterize(
            &terminal,
            &[],
            &mut HashMap::new(),
            "",
            0,
            None,
            &mut pixels,
        );
        assert_eq!(&pixels[..4], &[220, 220, 220, 255]);
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
