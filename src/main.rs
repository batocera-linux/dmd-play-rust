use chrono::{Local, NaiveDateTime, TimeDelta, TimeZone};
use clap::Parser;
use image::{
    codecs::gif::GifDecoder, imageops, io::Reader, AnimationDecoder, DynamicImage, Rgba, RgbaImage,
};
use std::{fs::File, io::BufReader, io::Write, net::TcpStream, path::Path, thread, time::Duration};

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
    #[arg(short, long, default_value = None)]
    file: Option<String>,
    /// text
    #[arg(short, long, default_value = None)]
    text: Option<String>,
    /// display current time
    #[arg(long, default_value_t = false)]
    clock: bool,
    /// clock: display only hours and minutes, no seconds
    #[arg(long, default_value_t = false)]
    no_seconds: bool,
    /// clock: strftime-formatted string (superseeds --h12 and --no-seconds)
    #[arg(long, default_value = None)]
    clock_format: Option<String>,
    /// clock: 12-hour format with AM and PM (default it 24h)
    #[arg(long, default_value_t = false)]
    h12: bool,
    /// display a countdown (2050-06-30 15:00:00)
    #[arg(long, default_value = None)]
    countdown: Option<String>,
    /// equivalent of changing all format with a prefix
    #[arg(long, default_value = None)]
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
    #[arg(short, long, default_value = None)]
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
    /// gradient image path
    #[arg(long, default_value = None)]
    gradient: Option<String>,
    /// for compatibility only
    #[arg(long, default_value_t = false)]
    no_fit: bool,
}

const DMD_HEADER_SIZE: usize = 10 + 1 + 4 + 2 + 2 + 1 + 1 + 4;

enum DMDLayer {
    MAIN,
    SECOND,
}

struct DmdConfig {
    width: u32,
    height: u32,
    header: [u8; DMD_HEADER_SIZE],
}

struct TextConfig<'a> {
    font_data: &'a [u8],
    gradient: &'a Option<DynamicImage>,
    text_color: Rgba<u8>,
    background_color: Rgba<u8>,
    text_align: imageutils::TextAlign,
    line_spacing: u8,
    moving_text: bool,
    fixed_text: bool,
    speed: u32,
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
    font_data: &[u8],
    line_spacing: u8,
    dmd_width: u32,
    dmd_height: u32,
    force_moving_text: bool,
) -> Result<(bool, u32), String> {
    let mut should_animate = false;
    let mut animation_new_width = dmd_width;

    let lines = text.split("\\n");
    let nlines = lines.clone().count() as u32;

    let accepable_ratio = 3.0;
    let all_spaces = line_spacing as u32 * (nlines - 1);
    let section_height = ((dmd_height - all_spaces) / nlines) as u32;
    let dmd_ratio = dmd_width as f32 / dmd_height as f32;

    for line in lines {
        let text_ratio = imageutils::get_text_ratio(line, font_data, section_height)?;

        let local_should_animate = text_ratio > dmd_ratio * accepable_ratio;
        if local_should_animate || force_moving_text {
            should_animate = true;
            let local_animation_new_width = (section_height as f32 * text_ratio) as u32;
            if local_animation_new_width > animation_new_width {
                animation_new_width = local_animation_new_width;
            }
        }
    }

    Ok((should_animate, animation_new_width))
}

fn get_dmd_animation_from_text(
    text: &str,
    dmd: &DmdConfig,
    txt: &TextConfig,
    text_width: u32,
) -> Result<(Vec<Box<[u8]>>, Vec<u32>), String> {
    let (dyn_img, start, real_width) = imageutils::generate_text_image(
        text,
        txt.font_data,
        txt.gradient,
        text_width,
        dmd.height,
        txt.background_color,
        txt.text_color,
        &txt.text_align,
        txt.line_spacing,
    )?;

    let mut frames_dmd = Vec::new();
    let mut frames_duration = Vec::new();

    for npixel in (0..dmd.width + (real_width - dmd.width) + dmd.width).rev() {
        let mut new_img = RgbaImage::new(dmd.width, dmd.height);
        imageutils::copy_image(
            &dyn_img,
            &mut new_img,
            npixel as i32 - start as i32 - real_width as i32,
            0,
        );
        let img565: Box<[u8]> = imageutils::image2dmdimage(
            &new_img,
            &imageutils::TextAlign::CENTER,
            dmd.width,
            dmd.height,
        )
        .map_err(|e| e.to_string())?;
        frames_dmd.push(img565);
        frames_duration.push(txt.speed);
    }

    Ok((frames_dmd, frames_duration))
}

fn send_image_text(
    client: &TcpStream,
    dmd: &DmdConfig,
    txt: &TextConfig,
    text: &str,
    once: bool,
) -> Result<bool, String> {
    let mut new_width = dmd.width;

    let (mut should_animate, animation_new_width) = is_text_to_animate(
        text,
        txt.font_data,
        txt.line_spacing,
        dmd.width,
        dmd.height,
        txt.moving_text,
    )?;

    if should_animate {
        new_width = animation_new_width;
    }

    if !txt.moving_text && txt.fixed_text {
        should_animate = false;
    }

    if should_animate {
        let (frames_dmd, frames_duration) =
            get_dmd_animation_from_text(text, dmd, txt, new_width)?;
        play_animation(dmd.header, client, &frames_dmd, frames_duration, once)?;
        Ok(true)
    } else {
        let (dyn_img, _start, _new_width) = imageutils::generate_text_image(
            text,
            txt.font_data,
            txt.gradient,
            dmd.width,
            dmd.height,
            txt.background_color,
            txt.text_color,
            &txt.text_align,
            txt.line_spacing,
        )?;

        let img565 = imageutils::image2dmdimage(&dyn_img, &txt.text_align, dmd.width, dmd.height)
            .map_err(|e| e.to_string())?;

        send_frame(client, dmd.header, &img565).map_err(|e| e.to_string())?;
        Ok(false)
    }
}

fn handle_case_file(
    dmd: &DmdConfig,
    client: &TcpStream,
    file: &str,
    once: bool,
) -> Result<bool, String> {
    let ext = Path::new(file)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    if ext == "gif" {
        send_image_file_gif(dmd, client, file, once)
    } else {
        send_image_file_basic(client, dmd, file)?;
        Ok(false)
    }
}

fn send_image_file_gif(
    dmd: &DmdConfig,
    client: &TcpStream,
    file: &str,
    once: bool,
) -> Result<bool, String> {
    let fd = File::open(file).map_err(|e| e.to_string())?;
    let reader = BufReader::new(fd);
    let decoder = GifDecoder::new(reader).map_err(|e| e.to_string())?;

    let frames = decoder.into_frames();
    let mut frames_dmd = Vec::new();
    let mut frames_duration = Vec::new();

    for frame in frames {
        let frame = frame.map_err(|e| e.to_string())?;
        let (x, y) = frame.delay().numer_denom_ms();
        let duration = (x as f32 / y as f32) as u32;

        let orig_img = frame.into_buffer();

        let img565: Box<[u8]> = imageutils::image2dmdimage(
            &orig_img,
            &imageutils::TextAlign::CENTER,
            dmd.width,
            dmd.height,
        )
        .map_err(|e| e.to_string())?;

        frames_dmd.push(img565);
        frames_duration.push(duration);
    }

    if frames_dmd.len() == 1 {
        send_frame(client, dmd.header, &frames_dmd[0]).map_err(|e| e.to_string())?;
        Ok(false)
    } else {
        play_animation(dmd.header, client, &frames_dmd, frames_duration, once)?;
        Ok(true)
    }
}

fn play_animation(
    header: [u8; DMD_HEADER_SIZE],
    client: &TcpStream,
    frames_dmd: &[Box<[u8]>],
    frames_duration: Vec<u32>,
    once: bool,
) -> Result<(), String> {
    loop {
        for (n, img565) in frames_dmd.iter().enumerate() {
            send_frame(client, header, img565).map_err(|e| e.to_string())?;
            thread::sleep(Duration::from_millis(frames_duration[n] as u64));
        }

        if once {
            return Ok(());
        }
    }
}

fn send_image_file_basic(
    client: &TcpStream,
    dmd: &DmdConfig,
    file: &str,
) -> Result<(), String> {
    let orig_img = Reader::open(file)
        .map_err(|e| e.to_string())?
        .decode()
        .map_err(|e| e.to_string())?;

    let img565: Box<[u8]> = imageutils::image2dmdimage(
        &orig_img,
        &imageutils::TextAlign::CENTER,
        dmd.width,
        dmd.height,
    )
    .map_err(|e| e)?;

    send_frame(client, dmd.header, &img565).map_err(|e| e.to_string())?;
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
    dmd: &DmdConfig,
    txt: &TextConfig,
    clock_format: Option<String>,
    h12: bool,
    no_seconds: bool,
) {
    let mut previous_txt = String::new();

    loop {
        let now = Local::now();

        let localtime = match clock_format {
            Some(ref x) => now.format(x).to_string(),
            None => {
                if h12 {
                    if no_seconds {
                        now.format("%-I:%M %p").to_string()
                    } else {
                        now.format("%-I:%M:%S %p").to_string()
                    }
                } else if no_seconds {
                    now.format("%H:%M").to_string()
                } else {
                    now.format("%H:%M:%S").to_string()
                }
            }
        };

        if previous_txt != localtime {
            previous_txt = localtime.clone();
            if let Err(e) = send_image_text(client, dmd, txt, &localtime, true) {
                eprintln!("{}", e);
            }
        }

        thread::sleep(Duration::from_millis(1000));
    }
}

fn handle_countdown(
    client: &TcpStream,
    dmd: &DmdConfig,
    txt: &TextConfig,
    countdown: String,
    countdown_header: Option<String>,
    countdown_format: String,
    countdown_format_0_minute: String,
    countdown_format_0_hour: String,
    countdown_format_0_day: String,
) -> Result<(), String> {
    let target = NaiveDateTime::parse_from_str(&countdown, "%Y-%m-%d %H:%M:%S")
        .map_err(|e| e.to_string())?;

    let target_datetime = Local
        .from_local_datetime(&target)
        .earliest()
        .ok_or_else(|| String::from("Error parsing countdown datetime"))?;

    let mut previous_txt = String::new();

    loop {
        let now = Local::now();
        let delta = (target_datetime - now).abs();
        let total_seconds = delta.num_seconds();

        let fmt = if total_seconds < 60 {
            &countdown_format_0_minute
        } else if total_seconds < 3600 {
            &countdown_format_0_hour
        } else if total_seconds < 86400 {
            &countdown_format_0_day
        } else {
            &countdown_format
        };

        let mut countdown_str = strfdelta(delta, fmt);

        if let Some(ref header) = countdown_header {
            countdown_str = header.to_owned() + "\\n" + &countdown_str;
        }

        if previous_txt != countdown_str {
            previous_txt = countdown_str.clone();
            if let Err(e) = send_image_text(client, dmd, txt, &countdown_str, true) {
                eprintln!("{}", e);
            }
        }

        thread::sleep(Duration::from_millis(1000));
    }
}

fn main() {
    let args = Cli::parse();
    let mut was_animation = false;

    let mut nplay = 0;
    if args.clear { nplay += 1; }
    if args.file.is_some() { nplay += 1; }
    if args.text.is_some() { nplay += 1; }
    if args.clock { nplay += 1; }
    if args.countdown.is_some() { nplay += 1; }

    if nplay == 0 {
        eprintln!("Missing something to play");
        return;
    }

    if nplay > 1 {
        eprintln!("Only one action required");
        return;
    }

    if args.moving_text && args.fixed_text {
        eprintln!("Warning: --moving-text and --fixed-text are both set; --moving-text takes precedence");
    }

    let server_address = format!("{}:{}", args.host, args.port);
    let client = match TcpStream::connect(&server_address) {
        Ok(stream) => stream,
        Err(e) => {
            eprintln!("Erreur de connexion au serveur: {}", e);
            return;
        }
    };

    let mut dmd_width = if args.hd { 256 } else { 128 };
    let mut dmd_height = if args.hd { 64 } else { 32 };

    if let Some(w) = args.width { dmd_width = w; }
    if let Some(h) = args.height { dmd_height = h; }

    let layer = if args.overlay { DMDLayer::SECOND } else { DMDLayer::MAIN };

    let background_color = Rgba([0, 0, 0, 255]);
    // alpha=0 is intentional: image2dmdimage ignores alpha (RGB565), and apply_gradient uses
    // alpha=0 as the text-pixel mask vs alpha=255 for background pixels
    let text_color = Rgba([args.red, args.green, args.blue, 0]);

    let header = get_header(
        dmd_width as u16,
        dmd_height as u16,
        layer,
        imageutils::get_dmd_buffer_size(dmd_width, dmd_height),
    );

    let dmd = DmdConfig {
        width: dmd_width,
        height: dmd_height,
        header,
    };

    let text_align = match args.align.as_deref() {
        Some("center") | None => imageutils::TextAlign::CENTER,
        Some("left") => imageutils::TextAlign::LEFT,
        Some("right") => imageutils::TextAlign::RIGHT,
        Some(_) => {
            eprintln!("Invalid alignment value, defaulting to center");
            imageutils::TextAlign::CENTER
        }
    };

    let gradient = match args.gradient {
        Some(ref gradient_path) => match Reader::open(gradient_path) {
            Ok(fd) => match fd.decode() {
                Ok(img) => Some(img.resize_exact(dmd_width, dmd_height, imageops::FilterType::Lanczos3)),
                Err(e) => {
                    eprintln!("unable to apply gradient: {}", e);
                    None
                }
            },
            Err(e) => {
                eprintln!("unable to apply gradient: {}", e);
                None
            }
        },
        None => None,
    };

    let font_data = match imageutils::load_font(&args.font) {
        Ok(data) => data,
        Err(e) => {
            eprintln!("{}", e);
            return;
        }
    };

    let txt = TextConfig {
        font_data: &font_data,
        gradient: &gradient,
        text_color,
        background_color,
        text_align,
        line_spacing: args.line_spacing,
        moving_text: args.moving_text,
        fixed_text: args.fixed_text,
        speed: args.speed,
    };

    if let Some(file) = args.file {
        match handle_case_file(&dmd, &client, &file, args.once) {
            Ok(x) => was_animation = x,
            Err(e) => eprintln!("{}", e),
        }
    }

    if let Some(text) = args.text {
        let dsp_text = if args.caps {
            text.to_uppercase().replace("\\N", "\\n")
        } else {
            text
        };
        match send_image_text(&client, &dmd, &txt, &dsp_text, args.once) {
            Ok(x) => was_animation = x,
            Err(e) => eprintln!("{}", e),
        }
    }

    if args.clock {
        handle_clock(
            &client,
            &dmd,
            &txt,
            args.clock_format,
            args.h12,
            args.no_seconds,
        );
    }

    if let Some(countdown) = args.countdown {
        if let Err(e) = handle_countdown(
            &client,
            &dmd,
            &txt,
            countdown,
            args.countdown_header,
            args.countdown_format,
            args.countdown_format_0_minute,
            args.countdown_format_0_hour,
            args.countdown_format_0_day,
        ) {
            eprintln!("{}", e);
        }
    }

    if args.clear {
        was_animation = true;
        let clear_txt = TextConfig {
            text_color: background_color,
            ..txt
        };
        if let Err(e) = send_image_text(&client, &dmd, &clear_txt, "", args.once) {
            eprintln!("{}", e);
        }
    }

    if args.overlay && !was_animation {
        thread::sleep(Duration::from_millis(args.overlay_time));
    }

    if let Err(e) = client.shutdown(std::net::Shutdown::Write) {
        eprintln!("{}", e);
    }
}
