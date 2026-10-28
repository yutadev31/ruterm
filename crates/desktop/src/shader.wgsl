@group(0) @binding(0) var terminal_texture: texture_2d<f32>;
@group(0) @binding(1) var terminal_sampler: sampler;

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> VertexOut {
    let positions = array<vec2<f32>, 3>(vec2(-1.0, -1.0), vec2(3.0, -1.0), vec2(-1.0, 3.0));
    var out: VertexOut;
    out.position = vec4(positions[index], 0.0, 1.0);
    out.uv = vec2((positions[index].x + 1.0) * 0.5, (1.0 - positions[index].y) * 0.5);
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let pixel = vec2<i32>(in.position.xy);
    let size = vec2<i32>(textureDimensions(terminal_texture));
    if (pixel.x >= size.x || pixel.y >= size.y) {
        return vec4(0.0, 0.0, 0.0, 1.0);
    }
    return textureLoad(terminal_texture, pixel, 0);
}
