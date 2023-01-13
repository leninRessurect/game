use std::io::{BufReader, Cursor};
use wgpu::util::DeviceExt;

use crate::{model, texture};

pub async fn load_string(file_name: &str) -> String {
    let path = std::path::Path::new(env!("OUT_DIR"))
        .join("res")
        .join(file_name);
    dbg!(&path);
    std::fs::read_to_string(path).unwrap()
}

pub async fn load_binary(file_name: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("OUT_DIR"))
        .join("res")
        .join(file_name);  
    
    std::fs::read(path).unwrap()
}

pub async fn load_texture(
    file_name: &str,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> texture::Texture {
    let bytes = load_binary(file_name).await;
    texture::Texture::from_bytes(device, queue, &bytes, file_name)
}

pub async fn load_model(
    file_name: &str,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    layout: &wgpu::BindGroupLayout,
) -> model::Model {
    let obj_text = load_string(file_name).await;
    let obj_cursor = Cursor::new(obj_text);
    let mut obj_reader = BufReader::new(obj_cursor);

    let (models, obj_materials) = tobj::load_obj_buf_async(
        &mut obj_reader, 
        &tobj::LoadOptions { 
            single_index: true, 
            triangulate: true, 
            ..Default::default()
        },
        |p| async move {
            let mat_text = load_string(&p).await;
            tobj::load_mtl_buf(&mut BufReader::new(Cursor::new(mat_text)))
        },
    ).await.unwrap();

    let mut materials: Vec<model::Material> = Vec::new();

    for m in obj_materials.unwrap() {
        dbg!(&m.diffuse_texture);
        let diffuse_texture = load_texture(
            &m.diffuse_texture, 
            device, 
            queue
        ).await;
        
        let bind_group = device.create_bind_group(
            &wgpu::BindGroupDescriptor {
                label: None,
                layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&diffuse_texture.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&diffuse_texture.sampler),
                    },
                ],
            }
        );

        materials.push(
            model::Material {
                name: m.name,
                diffuse_texture,
                bind_group,
            }
        );
    }

    let meshes = models.into_iter().map(|m| {
        let vertices = (0..m.mesh.positions.len() / 3).map(
            |i| model::ModelVertex {
                position: [
                    m.mesh.positions[i * 3],
                    m.mesh.positions[i * 3 + 1],
                    m.mesh.positions[i * 3 + 2],
                ],
                tex_coords: [
                    m.mesh.texcoords[i * 2],
                    m.mesh.texcoords[i * 2 + 1],
                ],
                normal: [
                    m.mesh.normals[i * 3],
                    m.mesh.normals[i * 3 + 1],
                    m.mesh.normals[i * 3 + 2],
                ],
            }
        ).collect::<Vec<_>>();

        let vertex_buffer = device.create_buffer_init(
            &wgpu::util::BufferInitDescriptor {
                label: Some(&format!("{:?} Index Buffer", file_name)),
                contents: bytemuck::cast_slice(&vertices),
                usage: wgpu::BufferUsages::VERTEX,
            }
        );

        let index_buffer = device.create_buffer_init(
            &wgpu::util::BufferInitDescriptor {
                label: Some(&format!("{:?} Index Buffer", file_name)),
                contents: bytemuck::cast_slice(&m.mesh.indices),
                usage: wgpu::BufferUsages::INDEX,
            }
        );

        model::Mesh {
            name: file_name.to_string(),
            vertex_buffer,
            index_buffer,
            num_elements: m.mesh.indices.len() as u32,
            material: m.mesh.material_id.unwrap_or(0),
        }

    }).collect::<Vec<_>>();

    model::Model { meshes, materials }
}