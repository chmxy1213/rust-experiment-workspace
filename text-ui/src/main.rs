use clap::Parser;
use figlet_rs::FIGfont;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Text to print
    #[arg(default_value = "vagent")]
    text: String,

    /// Font style (slant, standard, shadow, small)
    #[arg(short, long, default_value = "slant")]
    font: String,

    /// Version info to display in the bottom right corner
    #[arg(long)]
    info: Option<String>,
}

fn main() {
    let args = Args::parse();
    
    // Select the font data based on the argument
    let font_data = match args.font.as_str() {
        "standard" => include_str!("../fonts/standard.flf"),
        "shadow" => include_str!("../fonts/shadow.flf"),
        "small" => include_str!("../fonts/small.flf"),
        _ => include_str!("../fonts/slant.flf"),
    };
    
    // Parse the font
    let font = FIGfont::from_content(font_data).expect("Failed to parse font");
    
    // Convert text to ASCII art
    match font.convert(&args.text) {
        Some(figure) => {
            let output = figure.to_string();
            // Remove trailing newlines to keep control over spacing
            let trimmed_output = output.trim_end();
            println!("{}", trimmed_output);

            if let Some(info) = args.info {
                let lines: Vec<&str> = trimmed_output.lines().collect();
                if let Some(max_width) = lines.iter().map(|l| l.len()).max() {
                    // Check if we can fit the version on the last line?
                    // For now, let's print it on the next line, right aligned to the art.
                    if max_width >= info.len() {
                        // Using padding to align right
                        let padding = max_width - info.len();
                        println!("{:padding$}{}", "", info, padding = padding);
                    } else {
                        // If info is longer than the art, just print it
                        println!("{}", info);
                    }
                }
            }
        },
        None => eprintln!("Failed to convert text"),
    }
}
