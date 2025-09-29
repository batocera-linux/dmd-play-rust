use image::{imageops, DynamicImage, GenericImageView, Rgba, RgbaImage};
use imageproc::drawing::draw_text_mut;
use rusttype::{point, Font, Scale};
use std::{fs::read, path::Path};

pub enum TextAlign {
    CENTER,
    LEFT,
    RIGHT,
}

fn rgb888_to_rgb565(r: u8, g: u8, b: u8) -> u16 {
    let r5 = (r as u16) >> 3;
    let g6 = (g as u16) >> 2;
    let b5 = (b as u16) >> 3;
    (r5 << 11) | (g6 << 5) | b5
}

pub fn get_dmd_buffer_size(width: u32, height: u32) -> u32 {
    return (width * height * 3) as u32;
}

pub fn image2dmdimage<T: GenericImageView<Pixel = Rgba<u8>>>(
    orig_img: &T,
    text_align: &TextAlign,
    dmd_width: u32,
    dmd_height: u32,
) -> Result<Box<[u8]>, String> {
    // resize the image to something below 128x32
    let (orig_width, orig_height) = orig_img.dimensions();

    let new_width;
    let new_height;

    if (orig_width as f32 / orig_height as f32) < (dmd_width as f32 / dmd_height as f32) {
        new_height = dmd_height;
        new_width = (orig_width as f32 * new_height as f32 / orig_height as f32) as u32;
    } else {
        new_width = dmd_width;
        new_height = (orig_height as f32 * new_width as f32 / orig_width as f32) as u32;
    }

    let resized_img = imageops::resize(
        orig_img,
        new_width,
        new_height,
        imageops::FilterType::Lanczos3,
    );

    // create the dmd image
    let (width, height) = resized_img.dimensions();

    let mut bytes: Box<[u8]> =
        vec![0u8; get_dmd_buffer_size(dmd_width, dmd_height) as usize].into_boxed_slice();

    // init to 0
    for i in 0..bytes.len() {
        bytes[i] = 0;
    }

    let x_offset = match text_align {
        TextAlign::CENTER => (dmd_width - width) / 2,
        TextAlign::LEFT => 0,
        TextAlign::RIGHT => dmd_width - width,
    };

    let y_offset = (dmd_height - height) / 2;

    for y in 0..dmd_height {
        if y >= y_offset && y < (height + y_offset) {
            for x in 0..dmd_width {
                let idx_u32: u32 = ((y * dmd_width) + x) * 2;
                let idx: usize = idx_u32 as usize;
                if x >= x_offset && x < (width + x_offset) {
                    let pixel = resized_img.get_pixel(x - x_offset, y - y_offset);
                    let val: u16 = rgb888_to_rgb565(pixel[0], pixel[1], pixel[2]);
                    bytes[idx..idx + 2].copy_from_slice(&val.to_be_bytes());
                }
            }
        }
    }
    Ok(bytes)
}

// for an unknown reason, this compute a too large width. sum of advance_width is not the total size
fn get_text_width(font: &Font, scale: Scale, text: &str) -> u32 {
    let mut width = 0.0;
    let mut n = 0;
    let mut last_rsb: f32 = 0.0;

    for glyph in font.layout(text, scale, point(0.0, 0.0)) {
        // remove the left side bearing for the first letter LSB
        if n == 0 {
            width -= glyph.unpositioned().h_metrics().left_side_bearing;
        }

        let glyph_width = match glyph.pixel_bounding_box() {
            Some(x) => x.width() as f32,
            None => 0.0,
        };

        width += glyph.unpositioned().h_metrics().advance_width;
        last_rsb = glyph.unpositioned().h_metrics().advance_width
            - glyph.unpositioned().h_metrics().left_side_bearing
            - glyph_width;
        n = n + 1;
    }
    width = width - last_rsb;

    width.round() as u32
}

fn get_text_height(font: &Font, scale: Scale, text: &str) -> u32 {
    let mut miny = 0;
    let mut maxy = 0;

    for glyph in font.layout(text, scale, point(0.0, 0.0)) {
        if let Some(metrics) = glyph.pixel_bounding_box() {
            if metrics.max.y > maxy {
                maxy = metrics.max.y;
            }
            if metrics.min.y < miny {
                miny = metrics.min.y;
            }
        }
    }
    (maxy - miny) as u32
}

fn get_text_y(font: &Font, scale: Scale, text: &str) -> i32 {
    let v_metrics = font.v_metrics(scale);
    let mut maxy = 0;

    for glyph in font.layout(text, scale, point(0.0, 0.0)) {
        if let Some(metrics) = glyph.pixel_bounding_box() {
            if metrics.min.y < maxy {
                maxy = metrics.min.y;
            }
        }
    }
    -(v_metrics.ascent.ceil() as i32 + maxy)
}

fn get_text_x(font: &Font, scale: Scale, text: &str) -> i32 {
    for glyph in font.layout(text, scale, point(0.0, 0.0)) {
        // remove the left side bearing for the first letter
        return -glyph.unpositioned().h_metrics().left_side_bearing.round() as i32;
    }
    0
}

pub fn generate_text_image(
    text: &str,
    font_path: &str,
    gradient: &Option<DynamicImage>,
    width: u32,
    height: u32,
    background_color: Rgba<u8>,
    text_color: Rgba<u8>,
    text_align: &TextAlign,
    line_spacing: u8,
) -> Result<(DynamicImage, u32, u32), String> {
    let lines = text.split("\\n");
    let nlines = lines.clone().count() as u32;

    // single line
    if nlines == 1 {
        let (mut dyn_img, start, new_width) = generate_text_image_single_line(
            text,
            font_path,
            width,
            height,
            background_color,
            text_color,
            text_align,
        )?;

        match gradient {
            Some(x) => {
                dyn_img = apply_gradient(&dyn_img, &x);
            }
            None => {}
        }
        Ok((dyn_img, start, new_width))
    } else {
        // multiple lines
        let spaces = line_spacing as u32 * (nlines - 1);
        let section_height = (height - spaces) / nlines;
        let mut rgba_img = RgbaImage::new(width, height);
        let mut smallest_start = width - 1;
        let mut biggest_end = 0;

        let mut n: i32 = 0;
        for line in lines {
            let (dyn_img, start, new_width) = generate_text_image_single_line(
                line,
                font_path,
                width,
                section_height,
                background_color,
                text_color,
                text_align,
            )?;
            copy_image(
                &dyn_img,
                &mut rgba_img,
                0,
                section_height as i32 * n + n * line_spacing as i32,
            );
            if start < smallest_start {
                smallest_start = start;
            }
            if start + new_width > biggest_end {
                biggest_end = start + new_width;
            }
            n += 1;
        }

        let mut dyn_img = DynamicImage::ImageRgba8(rgba_img);

        match gradient {
            Some(x) => {
                dyn_img = apply_gradient(&dyn_img, &x);
            }
            None => {}
        }

        Ok((dyn_img, smallest_start, biggest_end - smallest_start))
    }
}

fn apply_gradient(img: &DynamicImage, gradient: &DynamicImage) -> DynamicImage {
    let width_img = img.width();
    let height_img = img.height();
    let width_gradient = gradient.width();
    let height_gradient = gradient.height();

    let mut new_img = RgbaImage::new(width_img, height_img);

    for y in 0..height_img {
        for x in 0..width_img {
            if x < width_gradient && y < height_gradient {
                let img_pixel = img.get_pixel(x, y);
                let gradient_pixel = gradient.get_pixel(x, y);
                let new_pixel;
                let min_value = 15;
                let max_alpha = 245;
                if (img_pixel[0] > min_value
                    || img_pixel[1] > min_value
                    || img_pixel[2] > min_value)
                    && img_pixel[3] < max_alpha
                {
                    new_pixel = Rgba([
                        gradient_pixel[0],
                        gradient_pixel[1],
                        gradient_pixel[2],
                        img_pixel[3],
                    ]);
                } else {
                    new_pixel = Rgba([0, 0, 0, 0]);
                }
                new_img.put_pixel(x, y, new_pixel);
            }
        }
    }
    return DynamicImage::ImageRgba8(new_img);
}

pub fn get_text_ratio(text: &str, font_path: &str, height: u32) -> Result<f32, String> {
    let font_data = match read(Path::new(&font_path)) {
        Ok(x) => x,
        Err(_) => return Err(String::from("Unable to read font")),
    };
    let font = match Font::try_from_bytes(&font_data) {
        Some(x) => x,
        None => return Err(String::from("Unable to read font")),
    };
    let scale = Scale::uniform((height * 5) as f32); // 5x for a nicer image (more precision)

    let genwidth = get_text_width(&font, scale, text);
    let genheight = get_text_height(&font, scale, text);

    return Ok(genwidth as f32 / genheight as f32);
}

fn generate_text_image_single_line(
    text: &str,
    font_path: &str,
    width: u32,
    height: u32,
    background_color: Rgba<u8>,
    text_color: Rgba<u8>,
    text_align: &TextAlign,
) -> Result<(DynamicImage, u32, u32), String> {
    let font_data = match read(Path::new(&font_path)) {
        Ok(x) => x,
        Err(_) => return Err(String::from("Unable to read font")),
    };
    let font = match Font::try_from_bytes(&font_data) {
        Some(x) => x,
        None => return Err(String::from("Unable to read font")),
    };
    let scale = Scale::uniform((height * 5) as f32); // 5x for a nicer image (more precision)

    let genwidth = get_text_width(&font, scale, text);
    let genheight = get_text_height(&font, scale, text);
    let img = RgbaImage::from_pixel(genwidth, genheight, background_color);

    let mut dyn_img = DynamicImage::ImageRgba8(img);
    let x = get_text_x(&font, scale, text);
    let y = get_text_y(&font, scale, text);

    draw_text_mut(&mut dyn_img, text_color, x, y, scale, &font, text);

    // hack: now, crop width cause we know that get_text_width returns too large (for an unknown reason)
    dyn_img = crop_width_right(&dyn_img)?;
    //dyn_img.save_with_format("x.png", ImageFormat::Png);

    let (rgba_img_fit, start, new_width) = resize_image_to_fit(&dyn_img, width, height, text_align);
    let dyn_img_fit = DynamicImage::ImageRgba8(rgba_img_fit);

    Ok((dyn_img_fit, start, new_width))
}

fn crop_width_right(dyn_img: &DynamicImage) -> Result<DynamicImage, String> {
    // compute the width we can reduce
    let width = dyn_img.width();
    let height = dyn_img.height();

    for x in (0..width).rev() {
        let mut found = false;
        for y in 0..height {
            let pixel = dyn_img.get_pixel(x, y);
            if pixel[0] != 0 || pixel[1] != 0 || pixel[2] != 0 {
                found = true;
            }
        }

        if found {
            // ok, can't reduce more, now crop
            let mut new_img = RgbaImage::new(x + 1, height);
            copy_image(&dyn_img, &mut new_img, 0, 0);
            return Ok(DynamicImage::ImageRgba8(new_img));
        }
    }

    Ok(dyn_img.clone())
}

pub fn copy_image(img_src: &DynamicImage, img_dst: &mut RgbaImage, x_offset: i32, y_offset: i32) {
    let width_src = img_src.width() as i32;
    let height_src = img_src.height() as i32;
    let width_dst = img_dst.width() as i32;
    let height_dst = img_dst.height() as i32;

    for y in 0..height_src {
        for x in 0..width_src {
            if x + x_offset >= 0
                && x + x_offset < width_dst
                && y + y_offset >= 0
                && y + y_offset < height_dst
            {
                let pixel = img_src.get_pixel(x as u32, y as u32);
                img_dst.put_pixel((x + x_offset) as u32, (y + y_offset) as u32, pixel);
            }
        }
    }
}

// fit the image to width/height. Return the image, the start point and the width
fn resize_image_to_fit(
    img: &DynamicImage,
    width: u32,
    height: u32,
    text_align: &TextAlign,
) -> (RgbaImage, u32, u32) {
    let width_img = img.width();
    let height_img = img.height();

    if width == width_img && height == height_img {
        return (img.to_rgba8(), 0, width);
    }

    let mut new_img = RgbaImage::new(width, height);

    if width_img as f32 / height_img as f32 > width as f32 / height as f32 {
        let new_width = width;
        let new_height = (height_img as f32 * new_width as f32 / width_img as f32) as u32;
        let reduced_img = img.resize_exact(new_width, new_height, imageops::FilterType::Lanczos3);
        copy_image(
            &reduced_img,
            &mut new_img,
            0,
            ((height - new_height) / 2) as i32,
        );
        (new_img, 0, new_width)
    } else {
        let new_height = height;
        let new_width = (width_img as f32 * new_height as f32 / height_img as f32) as u32;
        let reduced_img = img.resize_exact(new_width, new_height, imageops::FilterType::Lanczos3);
        let align_x = match text_align {
            TextAlign::CENTER => (width - new_width) / 2,
            TextAlign::LEFT => 0,
            TextAlign::RIGHT => width - new_width,
        };
        copy_image(&reduced_img, &mut new_img, align_x as i32, 0);
        (new_img, align_x, new_width)
    }
}
