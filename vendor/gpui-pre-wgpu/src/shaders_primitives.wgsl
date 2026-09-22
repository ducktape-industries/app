// --- shadows --- //

struct Shadow {
    order: u32,
    blur_radius: f32,
    // The shadow rect for drop shadows; the "hole" rect for inset shadows.
    bounds: Bounds,
    corner_radii: Corners,
    content_mask: Bounds,
    color: Hsla,
    // Only consulted when `inset == 1u`: the element's own bounds, used as a rounded-rect
    // clip so the shadow never escapes the element.
    element_bounds: Bounds,
    element_corner_radii: Corners,
    // 0 = drop shadow, 1 = inset shadow.
    inset: u32,
    pad: u32, // align to 8 bytes
}

struct ShadowVarying {
    @builtin(position) position: vec4<f32>,
    @location(0) @interpolate(flat) color: vec4<f32>,
    @location(1) @interpolate(flat) shadow_id: u32,
    //TODO: use `clip_distance` once Naga supports it
    @location(3) clip_distances: vec4<f32>,
}

@vertex
fn vs_shadow(@builtin(vertex_index) vertex_id: u32, @builtin(instance_index) instance_id: u32) -> ShadowVarying {
    let unit_vertex = vec2<f32>(f32(vertex_id & 1u), 0.5 * f32(vertex_id & 2u));
    var shadow = load_shadow(instance_id);

    var geometry: Bounds;
    if (shadow.inset != 0u) {
        geometry = shadow.element_bounds;
    } else {
        // Leave room for the gaussian tail outside the shadow rect.
        let margin = 3.0 * shadow.blur_radius;
        geometry = shadow.bounds;
        geometry.origin -= vec2<f32>(margin);
        geometry.size += 2.0 * vec2<f32>(margin);
    }

    var out = ShadowVarying();
    out.position = to_device_position(unit_vertex, geometry);
    out.color = hsla_to_rgba(shadow.color);
    out.shadow_id = instance_id;
    out.clip_distances = distance_from_clip_rect(unit_vertex, geometry, shadow.content_mask);
    return out;
}

@fragment
fn fs_shadow(input: ShadowVarying) -> @location(0) vec4<f32> {
    // Alpha clip first, since we don't have `clip_distance`.
    if (any(input.clip_distances < vec4<f32>(0.0))) {
        return vec4<f32>(0.0);
    }

    let shadow = load_shadow(input.shadow_id);
    let half_size = shadow.bounds.size / 2.0;
    let center = shadow.bounds.origin + half_size;
    let center_to_point = input.position.xy - center;

    let corner_radius = pick_corner_radius(center_to_point, shadow.corner_radii);

    var alpha: f32;
    if (shadow.blur_radius == 0.0) {
        let distance = quad_sdf(input.position.xy, shadow.bounds, shadow.corner_radii);
        alpha = saturate(0.5 - distance);
    } else {
        // The signal is only non-zero in a limited range, so don't waste samples
        let low = center_to_point.y - half_size.y;
        let high = center_to_point.y + half_size.y;
        let start = clamp(-3.0 * shadow.blur_radius, low, high);
        let end = clamp(3.0 * shadow.blur_radius, low, high);

        // Accumulate samples (we can get away with surprisingly few samples)
        let step = (end - start) / 4.0;
        var y = start + step * 0.5;
        alpha = 0.0;
        for (var i = 0; i < 4; i += 1) {
            let blur = blur_along_x(center_to_point.x, center_to_point.y - y,
                shadow.blur_radius, corner_radius, half_size);
            alpha +=  blur * gaussian(y, shadow.blur_radius) * step;
            y += step;
        }
    }

    if (shadow.inset != 0u) {
        // The inset shadow is the complement of the (blurred) hole rect, clipped to the element.
        // `saturate(0.5 - d)` gives a 1-pixel antialiased edge: d <= -0.5 -> 1, d >= 0.5 -> 0.
        alpha = 1.0 - alpha;
        let element_distance = quad_sdf(input.position.xy, shadow.element_bounds,
                                        shadow.element_corner_radii);
        alpha *= saturate(0.5 - element_distance);
    }

    return blend_color(input.color, alpha);
}

// --- path rasterization --- //

struct PathRasterizationVertex {
    xy_position: vec2<f32>,
    st_position: vec2<f32>,
    color: Background,
    bounds: Bounds,
}



struct PathRasterizationVarying {
    @builtin(position) position: vec4<f32>,
    @location(0) st_position: vec2<f32>,
    @location(1) @interpolate(flat) vertex_id: u32,
    //TODO: use `clip_distance` once Naga supports it
    @location(3) clip_distances: vec4<f32>,
}

@vertex
fn vs_path_rasterization(@builtin(vertex_index) vertex_id: u32) -> PathRasterizationVarying {
    let v = load_path_vertex(vertex_id);

    var out = PathRasterizationVarying();
    out.position = to_device_position_impl(v.xy_position);
    out.st_position = v.st_position;
    out.vertex_id = vertex_id;
    out.clip_distances = distance_from_clip_rect_impl(v.xy_position, v.bounds);
    return out;
}

@fragment
fn fs_path_rasterization(input: PathRasterizationVarying) -> @location(0) vec4<f32> {
    let dx = dpdx(input.st_position);
    let dy = dpdy(input.st_position);
    if (any(input.clip_distances < vec4<f32>(0.0))) {
        return vec4<f32>(0.0);
    }

    let v = load_path_vertex(input.vertex_id);
    let background = v.color;
    let bounds = v.bounds;

    var alpha: f32;
    if (length(vec2<f32>(dx.x, dy.x)) < 0.001) {
        // If the gradient is too small, return a solid color.
        alpha = 1.0;
    } else {
        let gradient = 2.0 * input.st_position.xx * vec2<f32>(dx.x, dy.x) - vec2<f32>(dx.y, dy.y);
        let f = input.st_position.x * input.st_position.x - input.st_position.y;
        let distance = f / length(gradient);
        alpha = saturate(0.5 - distance);
    }
    let prepared_gradient = prepare_gradient_color(
        background.tag,
        background.color_space,
        background.solid,
        background.colors,
    );
    let color = gradient_color(background, input.position.xy, bounds,
        prepared_gradient.solid, prepared_gradient.color0, prepared_gradient.color1);
    return vec4<f32>(color.rgb * color.a * alpha, color.a * alpha);
}

// --- paths --- //

struct PathSprite {
    bounds: Bounds,
}


struct PathVarying {
    @builtin(position) position: vec4<f32>,
    @location(0) texture_coords: vec2<f32>,
}

@vertex
fn vs_path(@builtin(vertex_index) vertex_id: u32, @builtin(instance_index) instance_id: u32) -> PathVarying {
    let unit_vertex = vec2<f32>(f32(vertex_id & 1u), 0.5 * f32(vertex_id & 2u));
    let sprite = load_path_sprite(instance_id);
    // Don't apply content mask because it was already accounted for when rasterizing the path.
    let device_position = to_device_position(unit_vertex, sprite.bounds);
    // For screen-space intermediate texture, convert screen position to texture coordinates
    let screen_position = sprite.bounds.origin + unit_vertex * sprite.bounds.size;
    let texture_coords = screen_position / globals.viewport_size;

    var out = PathVarying();
    out.position = device_position;
    out.texture_coords = texture_coords;

    return out;
}

@fragment
fn fs_path(input: PathVarying) -> @location(0) vec4<f32> {
    let sample = textureSample(t_sprite, s_sprite, input.texture_coords);
    return sample;
}

// --- underlines --- //

struct Underline {
    order: u32,
    pad: u32,
    bounds: Bounds,
    content_mask: Bounds,
    color: Hsla,
    thickness: f32,
    wavy: u32,
}


struct UnderlineVarying {
    @builtin(position) position: vec4<f32>,
    @location(0) @interpolate(flat) color: vec4<f32>,
    @location(1) @interpolate(flat) underline_id: u32,
    //TODO: use `clip_distance` once Naga supports it
    @location(3) clip_distances: vec4<f32>,
}

@vertex
fn vs_underline(@builtin(vertex_index) vertex_id: u32, @builtin(instance_index) instance_id: u32) -> UnderlineVarying {
    let unit_vertex = vec2<f32>(f32(vertex_id & 1u), 0.5 * f32(vertex_id & 2u));
    let underline = load_underline(instance_id);

    var out = UnderlineVarying();
    out.position = to_device_position(unit_vertex, underline.bounds);
    out.color = hsla_to_rgba(underline.color);
    out.underline_id = instance_id;
    out.clip_distances = distance_from_clip_rect(unit_vertex, underline.bounds, underline.content_mask);
    return out;
}

@fragment
fn fs_underline(input: UnderlineVarying) -> @location(0) vec4<f32> {
    const WAVE_FREQUENCY: f32 = 2.0;
    const WAVE_HEIGHT_RATIO: f32 = 0.8;

    // Alpha clip first, since we don't have `clip_distance`.
    if (any(input.clip_distances < vec4<f32>(0.0))) {
        return vec4<f32>(0.0);
    }

    let underline = load_underline(input.underline_id);
    if (underline.wavy == 0u)
    {
        return blend_color(input.color, input.color.a);
    }

    let half_thickness = underline.thickness * 0.5;

    let st = (input.position.xy - underline.bounds.origin) / underline.bounds.size.y - vec2<f32>(0.0, 0.5);
    let frequency = M_PI_F * WAVE_FREQUENCY * underline.thickness / underline.bounds.size.y;
    let amplitude = (underline.thickness * WAVE_HEIGHT_RATIO) / underline.bounds.size.y;

    let sine = sin(st.x * frequency) * amplitude;
    let dSine = cos(st.x * frequency) * amplitude * frequency;
    let distance = (st.y - sine) / sqrt(1.0 + dSine * dSine);
    let distance_in_pixels = distance * underline.bounds.size.y;
    let distance_from_top_border = distance_in_pixels - half_thickness;
    let distance_from_bottom_border = distance_in_pixels + half_thickness;
    let alpha = saturate(0.5 - max(-distance_from_bottom_border, distance_from_top_border));
    return blend_color(input.color, alpha * input.color.a);
}

// --- monochrome sprites --- //

struct MonochromeSprite {
    order: u32,
    pad: u32,
    bounds: Bounds,
    content_mask: Bounds,
    color: Hsla,
    tile: AtlasTile,
    transformation: TransformationMatrix,
}


struct MonoSpriteVarying {
    @builtin(position) position: vec4<f32>,
    @location(0) tile_position: vec2<f32>,
    @location(1) @interpolate(flat) color: vec4<f32>,
    @location(3) clip_distances: vec4<f32>,
}

@vertex
fn vs_mono_sprite(@builtin(vertex_index) vertex_id: u32, @builtin(instance_index) instance_id: u32) -> MonoSpriteVarying {
    let unit_vertex = vec2<f32>(f32(vertex_id & 1u), 0.5 * f32(vertex_id & 2u));
    let sprite = load_mono_sprite(instance_id);

    var out = MonoSpriteVarying();
    out.position = to_device_position_transformed(unit_vertex, sprite.bounds, sprite.transformation);

    out.tile_position = to_tile_position(unit_vertex, sprite.tile);
    out.color = hsla_to_rgba(sprite.color);
    out.clip_distances = distance_from_clip_rect_transformed(unit_vertex, sprite.bounds, sprite.content_mask, sprite.transformation);
    return out;
}

@fragment
fn fs_mono_sprite(input: MonoSpriteVarying) -> @location(0) vec4<f32> {
    let sample = textureSample(t_sprite, s_sprite, input.tile_position).r;
    let alpha_corrected = apply_contrast_and_gamma_correction(sample, input.color.rgb, gamma_params.grayscale_enhanced_contrast, gamma_params.gamma_ratios);

    // Alpha clip after using the derivatives.
    if (any(input.clip_distances < vec4<f32>(0.0))) {
        return vec4<f32>(0.0);
    }

    return blend_color(input.color, alpha_corrected);
}

// --- polychrome sprites --- //

struct PolychromeSprite {
    order: u32,
    pad: u32,
    grayscale: u32,
    opacity: f32,
    bounds: Bounds,
    content_mask: Bounds,
    corner_radii: Corners,
    tile: AtlasTile,
}


struct PolySpriteVarying {
    @builtin(position) position: vec4<f32>,
    @location(0) tile_position: vec2<f32>,
    @location(1) @interpolate(flat) sprite_id: u32,
    @location(3) clip_distances: vec4<f32>,
}

@vertex
fn vs_poly_sprite(@builtin(vertex_index) vertex_id: u32, @builtin(instance_index) instance_id: u32) -> PolySpriteVarying {
    let unit_vertex = vec2<f32>(f32(vertex_id & 1u), 0.5 * f32(vertex_id & 2u));
    let sprite = load_poly_sprite(instance_id);

    var out = PolySpriteVarying();
    out.position = to_device_position(unit_vertex, sprite.bounds);
    out.tile_position = to_tile_position(unit_vertex, sprite.tile);
    out.sprite_id = instance_id;
    out.clip_distances = distance_from_clip_rect(unit_vertex, sprite.bounds, sprite.content_mask);
    return out;
}

@fragment
fn fs_poly_sprite(input: PolySpriteVarying) -> @location(0) vec4<f32> {
    let sample = textureSample(t_sprite, s_sprite, input.tile_position);
    // Alpha clip after using the derivatives.
    if (any(input.clip_distances < vec4<f32>(0.0))) {
        return vec4<f32>(0.0);
    }

    let sprite = load_poly_sprite(input.sprite_id);
    let distance = quad_sdf(input.position.xy, sprite.bounds, sprite.corner_radii);

    var color = sample;
    if (sprite.grayscale != 0u) {
        let grayscale = dot(color.rgb, GRAYSCALE_FACTORS);
        color = vec4<f32>(vec3<f32>(grayscale), sample.a);
    }
    return blend_color(color, sprite.opacity * saturate(0.5 - distance));
}

// --- surfaces --- //

struct SurfaceParams {
    bounds: Bounds,
    content_mask: Bounds,
}

@group(1) @binding(0) var<uniform> surface_locals: SurfaceParams;
@group(1) @binding(1) var t_y: texture_2d<f32>;
@group(1) @binding(2) var t_cb_cr: texture_2d<f32>;
@group(1) @binding(3) var s_surface: sampler;

const ycbcr_to_RGB = mat4x4<f32>(
    vec4<f32>( 1.0000f,  1.0000f,  1.0000f, 0.0),
    vec4<f32>( 0.0000f, -0.3441f,  1.7720f, 0.0),
    vec4<f32>( 1.4020f, -0.7141f,  0.0000f, 0.0),
    vec4<f32>(-0.7010f,  0.5291f, -0.8860f, 1.0),
);

struct SurfaceVarying {
    @builtin(position) position: vec4<f32>,
    @location(0) texture_position: vec2<f32>,
    @location(3) clip_distances: vec4<f32>,
}

@vertex
fn vs_surface(@builtin(vertex_index) vertex_id: u32) -> SurfaceVarying {
    let unit_vertex = vec2<f32>(f32(vertex_id & 1u), 0.5 * f32(vertex_id & 2u));

    var out = SurfaceVarying();
    out.position = to_device_position(unit_vertex, surface_locals.bounds);
    out.texture_position = unit_vertex;
    out.clip_distances = distance_from_clip_rect(unit_vertex, surface_locals.bounds, surface_locals.content_mask);
    return out;
}

@fragment
fn fs_surface(input: SurfaceVarying) -> @location(0) vec4<f32> {
    // Alpha clip after using the derivatives.
    if (any(input.clip_distances < vec4<f32>(0.0))) {
        return vec4<f32>(0.0);
    }

    let y_cb_cr = vec4<f32>(
        textureSampleLevel(t_y, s_surface, input.texture_position, 0.0).r,
        textureSampleLevel(t_cb_cr, s_surface, input.texture_position, 0.0).rg,
        1.0);

    return ycbcr_to_RGB * y_cb_cr;
}
