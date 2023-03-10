use tracing::{error, info, warn};
use winit::{
    event::{ElementState, Event, KeyboardInput, VirtualKeyCode, WindowEvent, MouseButton, DeviceEvent},
    event_loop::{ControlFlow, EventLoop},
    window::{Window, WindowBuilder},
};
use wgpu::util::DeviceExt;
use cgmath::prelude::*;

mod texture;
mod model;
mod resources;
mod camera;

use model::{Vertex, DrawModel};
use camera::{Camera, CameraController, Projection};


#[repr(C)]
#[derive(Debug, Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct CameraUniform {
    // We can't use cgmath with bytemuck directly so we'll have
    // to convert the Matrix4 into a 4x4 f32 array
    view_position: [f32; 4],
    view_proj: [[f32; 4]; 4],
}

impl CameraUniform {
    fn new() -> Self {
        Self {
            view_position: [0.0; 4],
            view_proj: cgmath::Matrix4::identity().into(),
        }
    }

    fn update_view_proj(&mut self, camera: &Camera, projection: &camera::Projection) {
        // using Vector4 because of uniforms 16 byte spacing requirement
        self.view_position = camera.position.to_homogeneous().into();
        self.view_proj = (projection.calc_matrix() * camera.calc_matrix()).into();
    }
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct InstanceRaw {
    model: [[f32; 4]; 4],
    normal: [[f32; 3]; 3],
}

impl InstanceRaw {
    fn desc<'a>() -> wgpu::VertexBufferLayout<'a> {
        use std::mem;
        
        wgpu::VertexBufferLayout {
            array_stride: mem::size_of::<InstanceRaw>() as wgpu::BufferAddress,
            // We need to switch from using a step mode of Vertex to Instance
            // This means that our shaders will only change to use the next
            // instance when the shader starts processing a new instance
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &[
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x4,
                    offset: 0,
                    // While our vertex shader only uses locations 0, and 1 now,
                    // in later tutorials we'll be using 2, 3, and 4, for Vertex.
                    // We'll start at slot 5 not conflict with them later
                    shader_location: 5,
                },
                // A mat4 takes up 4 vertex slots as it is technically 4 vec4s. 
                // We need to define a slot for each vec4. 
                // We'll have to reassemble the mat4 in the shader.
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x4,
                    // offset by size of previous vertex attribute bytes
                    offset: mem::size_of::<[f32; 4]>() as wgpu::BufferAddress,
                    shader_location: 6,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x4,
                    offset: mem::size_of::<[f32; 8]>() as wgpu::BufferAddress,
                    shader_location: 7,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x4,
                    offset: mem::size_of::<[f32; 12]>() as wgpu::BufferAddress,
                    shader_location: 8,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x3,
                    offset: mem::size_of::<[f32; 16]>() as wgpu::BufferAddress,
                    shader_location: 9,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x3,
                    offset: mem::size_of::<[f32; 19]>() as wgpu::BufferAddress,
                    shader_location: 10,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x3,
                    offset: mem::size_of::<[f32; 22]>() as wgpu::BufferAddress,
                    shader_location: 11,
                },
            ]
        }
    }
}

struct Instance {
    position: cgmath::Vector3<f32>,
    rotation: cgmath::Quaternion<f32>,
}

impl Instance {
    fn to_raw(&self) -> InstanceRaw {
        let model = cgmath::Matrix4::from_translation(self.position) * 
            cgmath::Matrix4::from(self.rotation);
        InstanceRaw {
            model: model.into(),
            normal: cgmath::Matrix3::from(self.rotation).into(),
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct LightUniform {
    position: [f32; 3],
    // uniforms require 16 byte (4 float) spacing, 
    // so we need to use padding fields.
    _padding: u32,
    color: [f32; 3],
    _padding2: u32,
}

struct State {
    surface: wgpu::Surface,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    size: winit::dpi::PhysicalSize<u32>,
    clear_color: wgpu::Color,
    render_pipeline: wgpu::RenderPipeline,
    camera: Camera,
    projection: Projection,
    camera_controller: CameraController,
    camera_uniform: CameraUniform,
    camera_buffer: wgpu::Buffer,
    camera_bind_group: wgpu::BindGroup,
    instances: Vec<Instance>,
    instance_buffer: wgpu::Buffer,
    depth_texture: texture::Texture,
    obj_model: model::Model,
    light_uniform: LightUniform,
    light_buffer: wgpu::Buffer,
    light_bind_group: wgpu::BindGroup,
    light_render_pipeline: wgpu::RenderPipeline,
    debug_material: model::Material,
    mouse_pressed: bool,
}

fn create_render_pipeline(
    device: &wgpu::Device,
    shader: wgpu::ShaderModuleDescriptor,
    layout: &wgpu::PipelineLayout,
    vertex_layouts: &[wgpu::VertexBufferLayout],
    color_format: wgpu::TextureFormat,
    depth_format: Option<wgpu::TextureFormat>,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(shader);

    device.create_render_pipeline(
        &wgpu::RenderPipelineDescriptor { 
            label: Some("render Pipeline"), 
            layout: Some(layout), 
            vertex: wgpu::VertexState {

                module: &shader,
                entry_point: "vs_main",
                buffers: vertex_layouts,
            }, 
            fragment: Some(wgpu::FragmentState { 
                module: &shader, 
                entry_point: "fs_main", 
                targets: &[Some(wgpu::ColorTargetState {
                    format: color_format,
                    blend: Some(wgpu::BlendState {
                        alpha: wgpu::BlendComponent::REPLACE,
                        color: wgpu::BlendComponent::REPLACE,
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })], 
            }),
            primitive: wgpu::PrimitiveState { 
                topology: wgpu::PrimitiveTopology::TriangleList, 
                strip_index_format: None, 
                front_face: wgpu::FrontFace::Ccw, 
                cull_mode: Some(wgpu::Face::Back), 
                unclipped_depth: false, 
                polygon_mode: wgpu::PolygonMode::Fill, 
                conservative: false 
            }, 
            depth_stencil: depth_format.map(|format| wgpu::DepthStencilState {
                format,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }), 
            multisample: wgpu::MultisampleState { 
                count: 1, 
                mask: !0, 
                alpha_to_coverage_enabled: false 
            }, 
            multiview: None 
        }    
    )
}

impl State {
    // Creating some of the wgpu types requires async code
    async fn new(window: &Window) -> Self {
        let clear_color = wgpu::Color {
            r: 0.1,
            g: 0.2,
            b: 0.3,
            a: 1.0,
        };
        let size = window.inner_size();

        // The instance is a handle to our GPU
        // Backends::all => Vulkan + Metal + DX12 + Browser WebGPU
        let instance = wgpu::Instance::new(wgpu::Backends::all());
        let surface = unsafe { instance.create_surface(window) };
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::default(),
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .unwrap();

        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    features: wgpu::Features::empty(),
                    // WebGL doesn't support all of wgpu's features, so if
                    // we're building for the web we'll have to disable some.
                    limits: if cfg!(target_arch = "wasm32") {
                        wgpu::Limits::downlevel_webgl2_defaults()
                    } else {
                        wgpu::Limits::default()
                    },
                    label: None,
                },
                None, // Trace path
            )
            .await
            .unwrap();
        
        let texture_bind_group_layout = device.create_bind_group_layout(
            &wgpu::BindGroupLayoutDescriptor {
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture { 
                            sample_type: wgpu::TextureSampleType::Float { 
                                filterable: true 
                            },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(
                            wgpu::SamplerBindingType::Filtering
                        ),
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture { 
                            sample_type: wgpu::TextureSampleType::Float { 
                                filterable: true 
                            }, 
                            view_dimension: wgpu::TextureViewDimension::D2, 
                            multisampled: false 
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 3,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(
                            wgpu::SamplerBindingType::Filtering
                        ),
                        count: None,
                    },
                ],
                label: Some("texture_bind_group_layout"),
            }
        );

        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: surface.get_supported_formats(&adapter)[0],
            width: size.width,
            height: size.height,
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: wgpu::CompositeAlphaMode::Auto,
        };

        let camera = Camera::new(
            (0.0, 5.0, 10.0), 
            cgmath::Deg(-90.0),
            cgmath::Deg(-20.0)
        );

        let projection = camera::Projection::new(
            config.width,
            config.height,
            cgmath::Deg(45.0),
            0.1, 
            100.0
        );

        let camera_controller = camera::CameraController::new(
            4.0,
            0.4
        );

        let mut camera_uniform = CameraUniform::new();
        
        camera_uniform.update_view_proj(&camera, &projection);

        let camera_buffer = device.create_buffer_init(
            &wgpu::util::BufferInitDescriptor {
                label: Some("Camera Buffer"),
                contents: bytemuck::cast_slice(&[camera_uniform]),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            }
        );

        let camera_bind_group_layout = device.create_bind_group_layout(
            &wgpu::BindGroupLayoutDescriptor {
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::VERTEX | 
                            wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer { 
                            ty: wgpu::BufferBindingType::Uniform, 
                            has_dynamic_offset: false, 
                            min_binding_size: None, 
                        },
                        count: None,
                    },
                ],
                label: Some("camera_bind_group_layout"),
            }
        );

        let camera_bind_group = device.create_bind_group(
            &wgpu::BindGroupDescriptor {
                layout: &camera_bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: camera_buffer.as_entire_binding(),
                    },
                ],
                label: Some("camera_bind_group"),
            }
        );

        //------------- INSTANCES --------------
        const NUM_INSTANCES_PER_ROW: u32 = 10;
        const SPACE_BETWEEN: f32 = 3.0;
        let instances = (0..NUM_INSTANCES_PER_ROW).flat_map(
            |z| {
                (0..NUM_INSTANCES_PER_ROW).map(
                    move |x| {
                        let x = SPACE_BETWEEN * (
                            x as f32 - NUM_INSTANCES_PER_ROW as f32 / 2.0
                        );
                        let z = SPACE_BETWEEN * (
                            z as f32 - NUM_INSTANCES_PER_ROW as f32 / 2.0
                        );                        

                        let position = cgmath::Vector3 {
                            x: x,
                            y: 0.0,
                            z: z,
                        };

                        /* let rotation = if position.is_zero() {
                            // this is needed so an object at (0, 0, 0) won't get scaled to zero
                            // as Quaternions can effect scale if they're not created correctly
                            cgmath::Quaternion::from_axis_angle(
                                cgmath::Vector3::unit_z(), 
                                cgmath::Deg(0.0)
                            )
                        } else {
                            cgmath::Quaternion::from_axis_angle(
                                position.normalize(), 
                                cgmath::Deg(45.0)
                            )
                        }; */

                        let rotation = cgmath::Quaternion::from_axis_angle(
                            (0.0, 1.0, 0.0).into(), 
                            cgmath::Deg(180.0)
                        );

                        Instance {
                            position,
                            rotation,
                        }
                    }
                )
            }
        ).collect::<Vec<_>>();

        let instance_data = instances.iter().map(Instance::to_raw).collect::<Vec<_>>();
        
        let instance_buffer = device.create_buffer_init(
            &wgpu::util::BufferInitDescriptor {
                label: Some("Instance Buffer"),
                contents: bytemuck::cast_slice(&instance_data),
                usage: wgpu::BufferUsages::VERTEX,
            }
        );

        //------------- creating buffer to store light in ----------------
        let light_uniform = LightUniform {
            position: [2.0, 2.0, 2.0],
            _padding: 0,
            color: [1.0, 1.0, 1.0],
            _padding2: 0,
        };

        let light_buffer = device.create_buffer_init(
            &wgpu::util::BufferInitDescriptor {
                label: Some("Light Buffer"),
                contents: bytemuck::cast_slice(&[light_uniform]),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            }
        );

        let light_bind_group_layout = device.create_bind_group_layout(
            &wgpu::BindGroupLayoutDescriptor { 
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::VERTEX | 
                            wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer { 
                            ty: wgpu::BufferBindingType::Uniform, 
                            has_dynamic_offset: false, 
                            min_binding_size: None 
                        },
                        count: None,
                    },   
                ],
                label: Some("light_bind_group_layout"),
            }
        );

        let light_bind_group = device.create_bind_group(
            &wgpu::BindGroupDescriptor {
                label: Some("light_bind_group"),
                layout: &light_bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: light_buffer.as_entire_binding(),
                    },
                ],
            }
        );

        let render_pipeline_layout = device.create_pipeline_layout(
            &wgpu::PipelineLayoutDescriptor {
                label: Some("Render Pipeline Layout"),
                bind_group_layouts: &[
                    &texture_bind_group_layout,
                    &camera_bind_group_layout,
                    &light_bind_group_layout,
                ],
                push_constant_ranges: &[],
            }
        );

        let depth_texture = texture::Texture::create_depth_texture(
            &device, 
            &config, 
            "depth_texture"
        );

        let render_pipeline = {
            let shader = wgpu::ShaderModuleDescriptor {
                label: Some("Normal Shader"),
                source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
            };
            create_render_pipeline(
                &device, 
                shader, 
                &render_pipeline_layout, 
                &[model::ModelVertex::desc(), InstanceRaw::desc()], 
                config.format, 
                Some(texture::Texture::DEPTH_FORMAT),
            )
        };

        let light_render_pipeline = {
            let layout = device.create_pipeline_layout(
                &wgpu::PipelineLayoutDescriptor {
                    label: Some("Light Pipeline Layout"),
                    bind_group_layouts: &[
                        &camera_bind_group_layout, 
                        &light_bind_group_layout
                    ],
                    push_constant_ranges: &[],
                }
            );
            let shader = wgpu::ShaderModuleDescriptor {
                label: Some("Light Shader"),
                source: wgpu::ShaderSource::Wgsl(include_str!("light.wgsl").into()),
            };
            create_render_pipeline(
                &device, 
                shader, 
                &layout, 
                &[model::ModelVertex::desc()], 
                config.format, 
                Some(texture::Texture::DEPTH_FORMAT)
            )
        };

        surface.configure(&device, &config); 

        let obj_model = resources::load_model(
            "cube.obj", 
            &device, 
            &queue, 
            &texture_bind_group_layout
        ).await;

        let debug_material = {
            let diffuse_bytes = include_bytes!("../res/cobble-diffuse.png");
            let normal_bytes = include_bytes!("../res/cobble-normal.png");

            let diffuse_texture = texture::Texture::from_bytes(
                &device, 
                &queue, 
                diffuse_bytes, 
                "res/alt-diffuse.png", 
                false
            );
            let normal_texture = texture::Texture::from_bytes(
                &device, 
                &queue, 
                normal_bytes, 
                "res/alt-normal.png" ,
                true
            );

            model::Material::new(
                &device, 
                "alt-material", 
                diffuse_texture, 
                normal_texture, 
                &texture_bind_group_layout
            )
        };

        Self {
            surface,
            device,
            queue,
            config,
            size,
            clear_color,
            render_pipeline,
            camera,
            projection,
            camera_controller,
            camera_uniform,
            camera_buffer,
            camera_bind_group,
            instances,
            instance_buffer,
            depth_texture,
            obj_model,
            light_uniform,
            light_buffer,
            light_bind_group,
            light_render_pipeline,
            debug_material,
            mouse_pressed: false,
        }
    }

    fn resize(&mut self, new_size: winit::dpi::PhysicalSize<u32>) {
        if new_size.width > 0 && new_size.height > 0 {
            self.projection.resize(new_size.width, new_size.height);
            self.size = new_size;
            self.config.width = new_size.width;
            self.config.height = new_size.height;
            self.surface.configure(&self.device, &self.config);
        }
        self.depth_texture = texture::Texture::create_depth_texture(
            &self.device, 
            &self.config, 
            "depth_texture"
        );
    }

    fn input(&mut self, event: &WindowEvent) -> bool {
        match event {
            WindowEvent::CursorMoved {
                position,
                ..
            } => {
                let r = position.x / self.size.width as f64;
                let g = position.y / self.size.height as f64;
                self.clear_color = wgpu::Color {
                    r: r,
                    g: g,
                    b: 0.3,
                    a: 1.0,
                };
                true
            }

            WindowEvent::KeyboardInput { 
                input: KeyboardInput {
                    virtual_keycode: Some(key),
                    state,
                    ..
                }, 
                ..
            } => self.camera_controller.process_keyboard(*key, *state),

            WindowEvent::MouseWheel { delta, ..} => {
                self.camera_controller.process_scroll(delta);
                true
            }

            WindowEvent::MouseInput { 
                state, 
                button: MouseButton::Left, 
                .. 
            } => {
                self.mouse_pressed = *state == ElementState::Pressed;
                true
            }

            _ => false
        }
    }

    fn update(&mut self, dt: instant::Duration) {
        self.camera_controller.update_camera(&mut self.camera, dt);
        self.camera_uniform.update_view_proj(&self.camera, &self.projection);
        self.queue.write_buffer(
            &self.camera_buffer,
            0,
            bytemuck::cast_slice(&[self.camera_uniform]),
        );

        // update the light
        let old_light_position: cgmath::Vector3<_> = self.light_uniform.position.into();
        self.light_uniform.position = (
            cgmath::Quaternion::from_axis_angle(
                (0.0, 1.0, 0.0).into(),
                cgmath::Deg(60.0 * dt.as_secs_f32())
            ) * old_light_position
        ).into();
        self.queue.write_buffer(
            &self.light_buffer, 
            0, 
            bytemuck::cast_slice(&[self.light_uniform])
        );
    }

    fn render(&mut self) -> Result<(), wgpu::SurfaceError> {
        let output = self.surface.get_current_texture()?;
        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Render Encoder"),
            });

        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(self.clear_color),
                        store: true,
                    },
                })],
                depth_stencil_attachment: Some(
                    wgpu::RenderPassDepthStencilAttachment {
                        view: &self.depth_texture.view,
                        depth_ops: Some(
                            wgpu::Operations {
                                load: wgpu::LoadOp::Clear(1.0),
                                store: true,
                            }
                        ),
                        stencil_ops: None,
                    }
                ),
            });

            render_pass.set_vertex_buffer(1, self.instance_buffer.slice(..));

            use crate::model::DrawLight;
            render_pass.set_pipeline(&self.light_render_pipeline);
            render_pass.draw_light_model(
                &self.obj_model, 
                &self.camera_bind_group, 
                &self.light_bind_group
            );

            render_pass.set_pipeline(&self.render_pipeline);
            /* render_pass.draw_model_instanced(
                &self.obj_model, 
                0..self.instances.len() as u32, 
                &self.camera_bind_group,
                &self.light_bind_group,
            ); */
            render_pass.draw_model_instanced_with_material(
                &self.obj_model, 
                &self.debug_material,
                0..self.instances.len() as u32, 
                &self.camera_bind_group,
                &self.light_bind_group,
            );
  
        }

        // submit will accept anything that implements IntoIter
        self.queue.submit(std::iter::once(encoder.finish()));
        output.present();

        Ok(())
    }
}

pub async fn run() {
    tracing_subscriber::fmt::init();
    info!("info!!!");
    warn!("warning");
    error!("eeeeeek");
    let event_loop = EventLoop::new();
    let window = WindowBuilder::new()
        .with_title("Game")
        .build(&event_loop)
        .unwrap();

    let mut state = State::new(&window).await;
    let mut last_render_time = instant::Instant::now();

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;

        match event {
            Event::DeviceEvent { 
                event: DeviceEvent::MouseMotion { delta },
                .. 
            } => if state.mouse_pressed {
                state.camera_controller.process_mouse(delta.0, delta.1)
            }
            
            Event::WindowEvent {
                ref event,
                window_id,
            } if window_id == window.id() && !state.input(event) => {
                match event {
                    WindowEvent::CloseRequested
                    | WindowEvent::KeyboardInput {
                        input:
                            KeyboardInput {
                                state: ElementState::Pressed,
                                virtual_keycode: Some(VirtualKeyCode::Escape),
                                ..
                            },
                        ..
                    } => *control_flow = ControlFlow::Exit,

                    WindowEvent::Resized(physical_size) => {
                        state.resize(*physical_size);
                    }

                    WindowEvent::ScaleFactorChanged { new_inner_size, .. } => {
                        // new_inner_size is &&mut so we have to dereference it twice
                        state.resize(**new_inner_size);
                    }

                    _ => {}
                }
            }

            Event::RedrawRequested(window_id) if window_id == window.id() => {
                let now = instant::Instant::now();
                let dt = now - last_render_time;
                last_render_time = now;
                state.update(dt);
                match state.render() {
                    Ok(_) => {}
                    // reconfig surface if lost
                    Err(wgpu::SurfaceError::Lost) => state.resize(state.size),
                    // system out of memory -> quit
                    Err(wgpu::SurfaceError::OutOfMemory) => *control_flow = ControlFlow::Exit,
                    // all other errors to be resoled by next frame
                    Err(e) => eprintln!("{:?}", e),
                }
            }

            Event::MainEventsCleared => {
                // RedrawRequested will only trigger once, unless we manually
                // request it.
                window.request_redraw();
            }

            _ => {}
        }
    });
}
