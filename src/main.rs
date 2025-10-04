use chrono::{Local, NaiveDateTime, TimeDelta, TimeZone};
use clap::Parser;
use image::{
    codecs::gif::GifDecoder, imageops, io::Reader, AnimationDecoder, DynamicImage, Rgba, RgbaImage,
};
use std::{fs::File, io::BufReader, io::Write, net::TcpStream, thread, time::Duration};

mod imageutils;

#[derive(Parser)]
struct Cli {
    /// dmd server host
    #[arg(long, default_value = "localhost")]
    host: String,
    /// network connexion port
    #[arg(short, long, default_value_t = 6789)]
    port: u16,
    /// image path file
    #[arg(short, long, default_value=None)]
    file: Option<String>,
    /// text
    #[arg(short, long, default_value=None)]
    text: Option<String>,
    /// display current time
    #[arg(long, default_value_t = false)]
    clock: bool,
    /// clock: display only hours and minutes, no seconds
    #[arg(long, default_value_t = false)]
    no_seconds: bool,
    /// clock: strftime-formatted string (superseeds --h12 and --no-seconds)
    #[arg(long, default_value=None)]
    clock_format: Option<String>,
    /// clock: 12-hour format with AM and PM (default it 24h)
    #[arg(long, default_value_t = false)]
    h12: bool,
    /// display a countdown (2050-06-30 15:00:00)
    #[arg(long, default_value=None)]
    countdown: Option<String>,
    /// equivalent of changing all format with a prefix
    #[arg(long, default_value=None)]
    countdown_header: Option<String>,
    /// countdown format
    #[arg(long, default_value = "{D:2}d {H:2}:{M:02}:{S:02}")]
    countdown_format: String,
    /// countdown format when less than 1 day
    #[arg(long, default_value = "{H:2}:{M:02}:{S:02}")]
    countdown_format_0_day: String,
    /// countdown format when less than 1 hour
    #[arg(long, default_value = "{M:02}:{S:02}")]
    countdown_format_0_hour: String,
    /// countdown format when less than 1 minute
    #[arg(long, default_value = "{S:02}")]
    countdown_format_0_minute: String,
    /// path to the font file
    #[arg(long, default_value = "/usr/share/fonts/dejavu/DejaVuSans.ttf")]
    font: String,
    /// text alignment: center, left or right
    #[arg(short, long, default_value=None)]
    align: Option<String>,
    /// number of pixels between each line of text
    #[arg(short, long, default_value_t = 2)]
    line_spacing: u8,
    /// red text color level (0-255)
    #[arg(short, long, default_value_t = 255)]
    red: u8,
    /// green text color level (0-255)
    #[arg(short, long, default_value_t = 0)]
    green: u8,
    /// blue text color level (0-255)
    #[arg(short, long, default_value_t = 0)]
    blue: u8,
    /// don't loop forever
    #[arg(long, default_value_t = false)]
    once: bool,
    /// clear the screen
    #[arg(long, default_value_t = false)]
    clear: bool,
    /// restore the previous frames once finished
    #[arg(long, default_value_t = false)]
    overlay: bool,
    /// time to pause fixed images for the overlay in ms
    #[arg(long, default_value_t = 1000)]
    overlay_time: u64,
    /// convert text in all caps
    #[arg(long, default_value_t = false)]
    caps: bool,
    /// always makes the text to move, even if text fits
    #[arg(long, default_value_t = false)]
    moving_text: bool,
    /// never makes the text to move, prefer to adjust size
    #[arg(long, default_value_t = false)]
    fixed_text: bool,
    /// sleep time during each text position (in milliseconds)
    #[arg(short, long, default_value_t = 30)]
    speed: u32,
    /// hd format (256x64 dmd size)
    #[arg(long, default_value_t = false)]
    hd: bool,
    /// width
    #[arg(long, default_value = None)]
    width: Option<u32>,
    /// height
    #[arg(long, default_value = None)]
    height: Option<u32>,
    /// text alignment: center, left or right
    #[arg(long, default_value=None)]
    gradient: Option<String>,
    /// for compatibility only
    #[arg(long, default_value_t = false)]
    no_fit: bool,
}

// network package size
const DMD_HEADER_SIZE: usize = 10 + 1 + 4 + 2 + 2 + 1 + 1 + 4;

enum DMDLayer {
    MAIN,
    SECOND,
}

fn send_frame(
    mut client: &TcpStream,
    header: [u8; DMD_HEADER_SIZE],
    im: &[u8],
) -> Result<(), std::io::Error> {
    client.write_all(&header)?;
    client.write_all(im)?;
    client.flush()?;
    Ok(())
}

fn get_header(width: u16, height: u16, layer: DMDLayer, nbytes: u32) -> [u8; DMD_HEADER_SIZE] {
    let mut bytes: [u8; DMD_HEADER_SIZE] = [0; DMD_HEADER_SIZE];

    let version: u8 = 1;
    let keyword: &[u8] = "DMDStream".as_bytes();
    let mode: u32 = 3; // force rgb565
    let buffered: u8;
    let disconnect_others: u8;

    if matches!(layer, DMDLayer::MAIN) {
        buffered = 1;
        disconnect_others = 1;
    } else {
        buffered = 0;
        disconnect_others = 0;
    }

    let mut n = 0;
    let len = keyword.len();
    bytes[..len].copy_from_slice(keyword);
    n += len + 1;
    bytes[n] = version;
    n += 1;
    bytes[n..n + 4].copy_from_slice(&mode.to_be_bytes());
    n += 4;
    bytes[n..n + 2].copy_from_slice(&width.to_be_bytes());
    n += 2;
    bytes[n..n + 2].copy_from_slice(&height.to_be_bytes());
    n += 2;
    bytes[n] = buffered;
    n += 1;
    bytes[n] = disconnect_others;
    n += 1;
    bytes[n..n + 4].copy_from_slice(&nbytes.to_be_bytes());

    bytes
}

fn is_text_to_animate(
    text: &str,
    font_path: &str,
    line_spacing: u8,
    dmd_width: u32,
    dmd_height: u32,
    force_moving_text: bool,
) -> Result<(bool, u32), String> {
    let mut should_animate = false;
    let mut animation_new_width = dmd_width;

    let lines = text.split("\\n");
    let nlines = lines.clone().count() as u32;

    // animate if we use less than 1/3 of the height
    let accepable_ratio = 3.0;
    let all_spaces = line_spacing as u32 * (nlines - 1);
    let section_height = ((dmd_height - all_spaces) / nlines) as u32;
    let dmd_ratio = dmd_width as f32 / dmd_height as f32;

    for line in lines {
        let text_ratio = match imageutils::get_text_ratio(line, font_path, section_height) {
            Ok(x) => x,
            Err(e) => {
                return Err(e);
            }
        };

        // if at least one line require animation, then animate.
        let local_should_animate = text_ratio > dmd_ratio * accepable_ratio;
        if local_should_animate || force_moving_text {
            should_animate = true;
            let local_animation_new_width = (section_height as f32 * text_ratio) as u32;
            if local_animation_new_width > animation_new_width {
                animation_new_width = local_animation_new_width;
            }
        }
    }

    // when the text is to animate, compute the real part of the animation
    Ok((should_animate, animation_new_width))
}

fn get_dmd_animation_from_text(
    text: &str,
    font_path: &str,
    gradient: &Option<DynamicImage>,
    dmd_width: u32,
    dmd_height: u32,
    text_width: u32,
    background_color: Rgba<u8>,
    text_color: Rgba<u8>,
    text_align: &imageutils::TextAlign,
    line_spacing: u8,
    speed: u32,
) -> Result<(Vec<Box<[u8]>>, Vec<u32>), String> {
    let (dyn_img, start, real_width) = imageutils::generate_text_image(
        text,
        font_path,
        &gradient,
        text_width,
        dmd_height,
        background_color,
        text_color,
        text_align,
        line_spacing,
    )?;

    let mut frames_dmd = Vec::new();
    let mut frames_duration = Vec::new();

    for npixel in (0..dmd_width + (real_width - dmd_width) + dmd_width).rev() {
        let mut new_img = RgbaImage::new(dmd_width, dmd_height);
        imageutils::copy_image(
            &dyn_img,
            &mut new_img,
            npixel as i32 - start as i32 - real_width as i32,
            0,
        );
        let img565: Box<[u8]> = match imageutils::image2dmdimage(
            &new_img,
            &imageutils::TextAlign::CENTER,
            dmd_width,
            dmd_height,
        ) {
            Ok(img) => img,
            Err(e) => {
                return Err(e.to_string());
            }
        };
        frames_dmd.push(img565);
        frames_duration.push(speed);
    }

    Ok((frames_dmd, frames_duration))
}

fn send_image_text(
    client: &TcpStream,
    header: [u8; DMD_HEADER_SIZE],
    dmd_width: u32,
    dmd_height: u32,
    text: &str,
    font_path: &str,
    gradient: &Option<DynamicImage>,
    text_color: Rgba<u8>,
    background_color: Rgba<u8>,
    text_align: &imageutils::TextAlign,
    line_spacing: u8,
    force_moving_text: bool,
    force_fixed_text: bool,
    speed: u32,
    once: bool,
) -> Result<bool, String> {
    let mut new_width = dmd_width;

    let (mut should_animate, animation_new_width) = is_text_to_animate(
        text,
        font_path,
        line_spacing,
        dmd_width,
        dmd_height,
        force_moving_text,
    )?;

    if should_animate {
        new_width = animation_new_width;
    }

    // some options forces
    if force_moving_text == false && force_fixed_text {
        should_animate = false;
    }

    // play the animation, thus first, generate images, then play
    if should_animate {
        let (frames_dmd, frames_duration) = get_dmd_animation_from_text(
            text,
            font_path,
            &gradient,
            dmd_width,
            dmd_height,
            new_width,
            background_color,
            text_color,
            text_align,
            line_spacing,
            speed,
        )?;
        play_animation(header, &client, &frames_dmd, frames_duration, once)?;
        Ok(true)
    } else {
        let (dyn_img, _start, _new_width) = imageutils::generate_text_image(
            text,
            font_path,
            &gradient,
            dmd_width,
            dmd_height,
            background_color,
            text_color,
            text_align,
            line_spacing,
        )?;

        let img565 = match imageutils::image2dmdimage(&dyn_img, text_align, dmd_width, dmd_height) {
            Ok(x) => x,
            Err(e) => {
                return Err(e.to_string());
            }
        };

        match send_frame(&client, header, &img565) {
            Ok(_) => {}
            Err(e) => {
                return Err(e.to_string());
            }
        };
        Ok(false)
    }
}

fn handle_case_file(
    header: [u8; DMD_HEADER_SIZE],
    dmd_width: u32,
    dmd_height: u32,
    client: &TcpStream,
    file: String,
    once: bool,
) -> Result<bool, String> {
    if file.len() >= 4 && &file[file.len() - 4..] == ".gif" {
        send_image_file_gif(header, dmd_width, dmd_height, client, file, once)
    } else {
        send_image_file_basic(client, header, dmd_width, dmd_height, file)?;
        Ok(false)
    }
}

fn send_image_file_gif(
    header: [u8; DMD_HEADER_SIZE],
    dmd_width: u32,
    dmd_height: u32,
    client: &TcpStream,
    file: String,
    once: bool,
) -> Result<bool, String> {
    let fd = match File::open(file) {
        Ok(x) => x,
        Err(e) => return Err(e.to_string()),
    };
    let reader = BufReader::new(fd);
    let decoder = match GifDecoder::new(reader) {
        Ok(x) => x,
        Err(e) => {
            return Err(e.to_string());
        }
    };

    let frames = decoder.into_frames();
    let mut frames_dmd = Vec::new();
    let mut frames_duration = Vec::new();

    // build the animation array
    for frame in frames {
        let frame = match frame {
            Ok(x) => x,
            Err(e) => {
                return Err(e.to_string());
            }
        };
        let (x, y) = frame.delay().numer_denom_ms();
        let duration = (x as f32 / y as f32) as u32;

        let orig_img = frame.into_buffer();

        let img565: Box<[u8]> = match imageutils::image2dmdimage(
            &orig_img,
            &imageutils::TextAlign::CENTER,
            dmd_width,
            dmd_height,
        ) {
            Ok(img) => img,
            Err(e) => {
                return Err(e.to_string());
            }
        };

        frames_dmd.push(img565);
        frames_duration.push(duration);
    }

    if frames_dmd.len() == 1 {
        match send_frame(&client, header, &frames_dmd[0]) {
            Ok(_) => {}
            Err(e) => {
                return Err(e.to_string());
            }
        };
        Ok(false)
    } else {
        play_animation(header, &client, &frames_dmd, frames_duration, once)?;
        Ok(true)
    }
}

fn play_animation(
    header: [u8; DMD_HEADER_SIZE],
    client: &TcpStream,
    frames_dmd: &Vec<Box<[u8]>>,
    frames_duration: Vec<u32>,
    once: bool,
) -> Result<(), String> {
    let mut n;

    loop {
        n = 0;
        for img565 in frames_dmd {
            match send_frame(&client, header, &img565) {
                Ok(_) => {}
                Err(e) => {
                    return Err(e.to_string());
                }
            };

            thread::sleep(Duration::from_millis(frames_duration[n] as u64));
            n = n + 1;
        }

        if once {
            return Ok(());
        }
    }
}

fn send_image_file_basic(
    client: &TcpStream,
    header: [u8; DMD_HEADER_SIZE],
    dmd_width: u32,
    dmd_height: u32,
    file: String,
) -> Result<(), String> {
    let orig_img_code = match Reader::open(file) {
        Ok(x) => x,
        Err(e) => {
            return Err(e.to_string());
        }
    };

    let orig_img = match orig_img_code.decode() {
        Ok(x) => x,
        Err(e) => {
            return Err(e.to_string());
        }
    };

    let img565: Box<[u8]> = match imageutils::image2dmdimage(
        &orig_img,
        &imageutils::TextAlign::CENTER,
        dmd_width,
        dmd_height,
    ) {
        Ok(img) => img,
        Err(e) => {
            return Err(e);
        }
    };

    match send_frame(&client, header, &img565) {
        Ok(_) => {}
        Err(e) => {
            return Err(e.to_string());
        }
    };

    Ok(())
}

fn strfdelta(duration: TimeDelta, format: &str) -> String {
    let total_seconds = duration.num_seconds();
    let days = total_seconds / 86400;
    let remaining_seconds = total_seconds % 86400;
    let hours = remaining_seconds / 3600;
    let remaining_seconds = remaining_seconds % 3600;
    let minutes = remaining_seconds / 60;
    let seconds = remaining_seconds % 60;

    format
        .replace("{D:2}", &format!("{:02}", days))
        .replace("{D}", &days.to_string())
        .replace("{H:2}", &format!("{:02}", hours))
        .replace("{H}", &hours.to_string())
        .replace("{M:02}", &format!("{:02}", minutes))
        .replace("{M}", &minutes.to_string())
        .replace("{S:02}", &format!("{:02}", seconds))
        .replace("{S}", &seconds.to_string())
}

fn handle_clock(
    client: &TcpStream,
    header: [u8; DMD_HEADER_SIZE],
    dmd_width: u32,
    dmd_height: u32,
    font_path: &str,
    gradient: &Option<DynamicImage>,
    text_color: Rgba<u8>,
    background_color: Rgba<u8>,
    text_align: &imageutils::TextAlign,
    line_spacing: u8,
    moving_text: bool,
    fixed_text: bool,
    speed: u32,
    clock_format: Option<String>,
    h12: bool,
    no_seconds: bool,
) {
    let mut previous_txt = String::new();
    let mut localtime;

    loop {
        let now = Local::now();

        match clock_format {
            Some(ref x) => {
                localtime = now.format(&x).to_string();
            }
            None => {
                if h12 {
                    if no_seconds {
                        localtime = now.format("%-I:%M %p").to_string();
                    } else {
                        localtime = now.format("%-I:%M:%S %p").to_string();
                    }
                } else {
                    if no_seconds {
                        localtime = now.format("%H:%M").to_string();
                    } else {
                        localtime = now.format("%H:%M:%S").to_string();
                    }
                }
            }
        }

        if previous_txt != localtime {
            previous_txt = localtime.clone();

            let _ = match send_image_text(
                &client,
                header,
                dmd_width,
                dmd_height,
                &localtime,
                &font_path,
                &gradient,
                text_color,
                background_color,
                &text_align,
                line_spacing,
                moving_text,
                fixed_text,
                speed,
                true,
            ) {
                Ok(_) => {}
                Err(e) => {
                    eprintln!("{}", e.to_string());
                }
            };
        }

        thread::sleep(Duration::from_millis(1000));
    }
}

fn handle_countdown(
    client: &TcpStream,
    header: [u8; DMD_HEADER_SIZE],
    dmd_width: u32,
    dmd_height: u32,
    font_path: &str,
    gradient: &Option<DynamicImage>,
    text_color: Rgba<u8>,
    background_color: Rgba<u8>,
    text_align: &imageutils::TextAlign,
    line_spacing: u8,
    moving_text: bool,
    fixed_text: bool,
    speed: u32,
    countdown: String,
    countdown_header: Option<String>,
    countdown_format: String,
    countdown_format_0_minute: String,
    countdown_format_0_hour: String,
    countdown_format_0_day: String,
) -> Result<(), String> {
    match NaiveDateTime::parse_from_str(&countdown.to_string(), "%Y-%m-%d %H:%M:%S") {
        Ok(target) => {
            let mut previous_txt = String::new();
            let mut countdown_str: String;

            let target_datetime = match Local.from_local_datetime(&target).earliest() {
                Some(x) => x,
                None => {
                    return Err(String::from("Error parsing"));
                }
            };

            loop {
                let now = Local::now();

                let delta = (target_datetime - now).abs();
                let total_seconds = delta.num_seconds();

                if (total_seconds >= 0 && total_seconds < 60)
                    || (total_seconds < 0 && total_seconds > -60)
                {
                    countdown_str = strfdelta(delta, &countdown_format_0_minute.to_string());
                } else if (total_seconds > 0 && total_seconds < 3600)
                    || (total_seconds < 0 && total_seconds > -3600)
                {
                    countdown_str = strfdelta(delta, &countdown_format_0_hour.to_string());
                } else if (total_seconds > 0 && total_seconds < 86400)
                    || (total_seconds < 0 && total_seconds > -86400)
                {
                    countdown_str = strfdelta(delta, &countdown_format_0_day.to_string());
                } else {
                    countdown_str = strfdelta(delta, &countdown_format.to_string());
                }
                match countdown_header {
                    Some(ref countdown_header) => {
                        countdown_str = countdown_header.to_owned() + "\\n" + &countdown_str;
                    }
                    None => {}
                }

                if previous_txt != countdown_str {
                    previous_txt = countdown_str.clone();

                    let _ = match send_image_text(
                        &client,
                        header,
                        dmd_width,
                        dmd_height,
                        &countdown_str,
                        &font_path,
                        &gradient,
                        text_color,
                        background_color,
                        &text_align,
                        line_spacing,
                        moving_text,
                        fixed_text,
                        speed,
                        true,
                    ) {
                        Ok(_) => {}
                        Err(e) => {
                            eprintln!("{}", e.to_string());
                        }
                    };
                }

                thread::sleep(Duration::from_millis(1000));
            }
        }
        Err(e) => {
            return Err(e.to_string());
        }
    }
}

fn main() {
    let args = Cli::parse();
    let mut was_animation = false; // set to true to disable overlay sleep time at the end

    // at least one
    let mut nplay = 0;
    if args.clear {
        nplay += 1;
    }
    if args.file.is_some() {
        nplay += 1;
    }
    if args.text.is_some() {
        nplay += 1;
    }
    if args.clock {
        nplay += 1;
    }
    if args.countdown.is_some() {
        nplay += 1;
    }

    if nplay == 0 {
        eprintln!("Missing something to play");
        return;
    }

    if nplay > 1 {
        eprintln!("Only one action required");
        return;
    }

    let server_address = format!("{}:{}", args.host, args.port);
    let client = match TcpStream::connect(server_address) {
        Ok(stream) => stream,
        Err(e) => {
            eprintln!("Erreur de connexion au serveur: {}", e);
            return;
        }
    };

    //
    let mut layer = DMDLayer::MAIN;

    let mut dmd_width;
    let mut dmd_height;

    if args.hd {
        dmd_width = 256;
        dmd_height = 64;
    } else {
        dmd_width = 128;
        dmd_height = 32;
    }

    match args.width {
        Some(x) => {
            dmd_width = x;
        }
        None => {}
    };

    match args.height {
        Some(x) => {
            dmd_height = x;
        }
        None => {}
    };

    if args.overlay {
        layer = DMDLayer::SECOND;
    }

    let background_color = Rgba([0, 0, 0, 255]);
    let text_color = Rgba([args.red, args.green, args.blue, 0]);

    // compute the header only once while it is always the same one
    let header = get_header(
        dmd_width as u16,
        dmd_height as u16,
        layer,
        imageutils::get_dmd_buffer_size(dmd_width, dmd_height),
    );

    let text_align;

    match args.align {
        Some(align) => match align.as_str() {
            "center" => text_align = imageutils::TextAlign::CENTER,
            "left" => text_align = imageutils::TextAlign::LEFT,
            "right" => text_align = imageutils::TextAlign::RIGHT,
            _ => {
                eprintln!("Invalid alignement value");
                text_align = imageutils::TextAlign::CENTER;
            }
        },
        None => {
            text_align = imageutils::TextAlign::CENTER;
        }
    };

    let gradient = match args.gradient {
        Some(gradient_path) => match Reader::open(gradient_path) {
            Ok(gradient_fd) => match gradient_fd.decode() {
                Ok(img) => {
                    Some(img.resize_exact(dmd_width, dmd_height, imageops::FilterType::Lanczos3))
                }
                Err(e) => {
                    eprintln!("unable to apply gradient: {}", e.to_string());
                    None
                }
            },
            Err(e) => {
                eprintln!("unable to apply gradient: {}", e.to_string());
                None
            }
        },
        None => None,
    };

    match args.file {
        Some(file) => {
            let _ = match handle_case_file(header, dmd_width, dmd_height, &client, file, args.once)
            {
                Ok(x) => {
                    was_animation = x;
                }
                Err(e) => {
                    eprintln!("{}", e.to_string());
                }
            };
        }
        None => {}
    };

    match args.text {
        Some(text) => {
            let mut dsp_text = text.clone();
            if args.caps {
                dsp_text = text.to_uppercase().replace("\\N", "\\n");
            }
            let _ = match send_image_text(
                &client,
                header,
                dmd_width,
                dmd_height,
                &dsp_text,
                &args.font,
                &gradient,
                text_color,
                background_color,
                &text_align,
                args.line_spacing,
                args.moving_text,
                args.fixed_text,
                args.speed,
                args.once,
            ) {
                Ok(x) => {
                    was_animation = x;
                }
                Err(e) => {
                    eprintln!("{}", e.to_string());
                }
            };
        }
        None => {}
    };

    if args.clock {
        handle_clock(
            &client,
            header,
            dmd_width,
            dmd_height,
            &args.font,
            &gradient,
            text_color,
            background_color,
            &text_align,
            args.line_spacing,
            args.moving_text,
            args.fixed_text,
            args.speed,
            args.clock_format,
            args.h12,
            args.no_seconds,
        );
    }

    match args.countdown {
        Some(countdown) => {
            match handle_countdown(
                &client,
                header,
                dmd_width,
                dmd_height,
                &args.font,
                &gradient,
                text_color,
                background_color,
                &text_align,
                args.line_spacing,
                args.moving_text,
                args.fixed_text,
                args.speed,
                countdown,
                args.countdown_header,
                args.countdown_format,
                args.countdown_format_0_minute,
                args.countdown_format_0_hour,
                args.countdown_format_0_day,
            ) {
                Ok(_) => {}
                Err(e) => {
                    eprintln!("{}", e.to_string());
                }
            }
        }
        None => {}
    };

    if args.clear {
        was_animation = true;

        let _ = match send_image_text(
            &client,
            header,
            dmd_width,
            dmd_height,
            "",
            &args.font,
            &gradient,
            background_color,
            background_color,
            &imageutils::TextAlign::CENTER,
            0,
            args.moving_text,
            args.fixed_text,
            args.speed,
            args.once,
        ) {
            Ok(_) => {}
            Err(e) => {
                eprintln!("{}", e.to_string());
            }
        };
    }

    // at the end, if we have overlay, we sleep
    if args.overlay && was_animation == false {
        thread::sleep(Duration::from_millis(args.overlay_time));
    }

    let _ = match client.shutdown(std::net::Shutdown::Write) {
        Ok(_) => {}
        Err(e) => {
            eprintln!("{}", e.to_string());
        }
    };
}
