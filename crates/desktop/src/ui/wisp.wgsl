const TAU: f32 = 6.2831853;

fn disc(p: vec2<f32>, radius: f32, aa: f32) -> f32 {
    return 1.0 - smoothstep(radius - aa, radius + aa, length(p));
}

fn effect(in: EffectInput) -> vec4<f32> {
    let time = param(0).x;
    let thinking = param(0).y;
    let working = param(0).z;
    let writing = param(0).w;
    let connecting = param(1).x;
    let compacting = param(1).y;
    let waiting = param(1).z;
    let expanded = param(1).w > 0.5;
    let fill = sample_content(in.size * 0.5);
    let tint = fill.rgb / max(fill.a, 1e-4);
    let radius = min(in.size.x, in.size.y) * 0.5;
    let aa = max(0.015, 0.7 / (radius * scale_factor()));
    let centred = (in.local - in.size * 0.5) / radius;
    let p = vec2<f32>(centred.x, -centred.y);
    let flow = working + writing;
    let gathering = thinking + compacting;
    let breath = sin(time * TAU / 2.6);
    let rise = 0.24 * thinking + 0.48 * working + 0.68 * writing + 0.12 * compacting;
    let height = max(p.y + 0.2, 0.0);
    let detail = select(0.0, 0.018 * sin(height * 9.0 - time * TAU / 2.6) * height, expanded);
    let sway = (0.055 * (thinking + working) + 0.025 * writing)
        * sin(height * 4.0 - time * TAU / 2.6) * height + detail * (gathering + flow);
    let taper = 1.0 + height * height * (1.8 * working + 2.8 * writing + thinking);
    let squash = 1.0 - rise;
    let centre = vec2<f32>((p.x - sway) * taper, (p.y + 0.15) * squash);
    let stream = 0.025 * writing * sin(height * 7.0 - time * TAU / 2.6) * height;
    let body_radius = 0.34 + 0.015 * breath * gathering - 0.05 * compacting + stream;
    let distance = length(centre);
    let body = 1.0 - smoothstep(body_radius - aa, body_radius + aa, distance);
    let core = disc(centre + vec2<f32>(0.0, 0.06), body_radius * 0.45, aa);
    let halo = 0.12 * (1.0 - smoothstep(body_radius, body_radius + 0.16, distance)) * (1.0 - body);

    var particles = 0.0;
    if waiting < 0.999 && connecting < 0.999 {
        let count = select(2, 5, expanded);
        for (var index = 0; index < count; index++) {
            let seed = f32(index) / f32(count);
            let age = fract(time / 2.6 + seed);
            let envelope = sin(age * 3.14159265);
            let angle = seed * TAU + age * 1.2;
            let orbit = mix(0.85, 0.18, age);
            let inward = vec2<f32>(cos(angle), sin(angle) * 0.65) * orbit;
            let outward = vec2<f32>((seed - 0.4) * (0.3 + age), -0.05 + age * 0.95);
            let at = mix(inward, outward, clamp(flow, 0.0, 1.0));
            particles += disc(p - at, select(0.08, 0.045, expanded), aa) * envelope;
        }
    }
    particles *= clamp(gathering + working, 0.0, 1.0) * 0.55;

    let around = atan2(p.y, p.x);
    let orbit = 1.0 - smoothstep(aa, aa * 2.0, abs(length(p) - 0.72));
    let arc = smoothstep(-0.3, 0.1, sin(around - time * TAU / 3.8));
    var circle = orbit * arc * connecting * 0.7;
    if expanded {
        let q = (p + vec2<f32>(0.0, 0.57)) / vec2<f32>(0.8, 0.22);
        let ring = 1.0 - smoothstep(0.035, 0.035 + aa * 2.0, abs(length(q) - 1.0));
        let turn = 0.7 + 0.3 * sin(atan2(q.y, q.x) - time * TAU / 5.2);
        circle += ring * turn * (gathering + flow) * 0.24;
    }
    let compression = (1.0 - smoothstep(aa, aa * 2.0, abs(abs(p.x) - (0.6 + 0.035 * breath))))
        * (1.0 - smoothstep(0.18, 0.3, abs(p.y + 0.12))) * compacting * 0.65;
    let attention = disc(p - vec2<f32>(0.5, 0.45), 0.13, aa) * waiting;
    let bare = 1.0 - body;
    let secondary = clamp(particles + circle + compression + attention, 0.0, 1.0) * bare;
    let alpha = clamp(body + halo + secondary, 0.0, 1.0);
    let light = mix(tint, vec3<f32>(1.0), 0.24);
    let color = tint * (body + halo + secondary) + (light - tint) * core * body;
    return vec4<f32>(color, alpha) * fill.a;
}
