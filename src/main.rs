use std::io::{BufWriter, Write};

fn output_flag(args: &[String]) -> Option<String> {
    let mut i = 0;
    while i < args.len() {
        if args[i] == "-o" || args[i] == "--output" {
            return args.get(i + 1).cloned();
        }
        i += 1;
    }
    None
}

fn delete_raw_flag(args: &[String]) -> bool {
    args.iter().any(|a| a == "--delete-raw")
}

fn frames_flag(args: &[String]) -> bool {
    args.iter().any(|a| a == "--frames")
}

fn ok_or_exit(result: std::io::Result<()>, op: &str) {
    if let Err(e) = result {
        eprintln!("ulp: {op}: {e}");
        std::process::exit(1);
    }
}

fn open_output(out_path: Option<&str>) -> std::io::Result<Box<dyn Write>> {
    match out_path {
        Some(p) => Ok(Box::new(BufWriter::with_capacity(
            8 << 20,
            std::fs::File::create(p)?,
        ))),
        None => Ok(Box::new(BufWriter::with_capacity(
            8 << 20,
            std::io::stdout().lock(),
        ))),
    }
}

fn open_output_or_exit(out_path: Option<&str>) -> Box<dyn Write> {
    match open_output(out_path) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("ulp: create output file: {e}");
            std::process::exit(1);
        }
    }
}

const HELP: &str = "ulp — Unified Leak Pool\n\
index huge url:user:pass combo lists into a smaller .ulp store that answers\n\
exact url/user/pass lookups in microseconds.\n\
\n\
usage: ulp <subcommand> [args]\n\
       ulp --help | --version\n\
\n\
subcommands:\n\
  build      <out.ulp> <in1.txt> [in2.txt ...] [--delete-raw]\n\
  append     <db.ulp> <new1.txt> [new2.txt ...] [--delete-raw]\n\
  repair     <file.ulp>\n\
  guide      <out.ulp> <folder-or-files...> [--delete-raw]\n\
  query      <file.ulp> <url|user|pass> <value> [-o out.txt]\n\
  match      <file.ulp> <contains|prefix> <keyword> [--frames] [-o out.txt]\n\
             <file.ulp> user <prefix|suffix> <keyword> [-o out.txt]\n\
  dump       <file.ulp> [--frames] [-o out.txt]\n\
  info       <file.ulp> [--json]\n\
  bench      <file.ulp> <url|user|pass> <value> <N>\n\
  norm       <in1.txt> [in2.txt ...]\n\
  merge      <in.ulp> <out.ulp>\n\
  sortcount  <in.ulp>\n\
\n\
options:\n\
  -h, --help      print this help and exit\n\
  -V, --version   print bin + crate + format version and exit\n\
";

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: ulp <build|query|dump|info> ...");
        std::process::exit(2);
    }
    match args[1].as_str() {
        "-h" | "--help" | "help" => print!("{HELP}"),
        "-V" | "--version" | "version" => {
            println!(
                "ulp {} (format {})",
                env!("CARGO_PKG_VERSION"),
                ulp::FORMAT_VERSION
            );
        }
        "build" => {
            if args.len() < 4 {
                eprintln!("usage: ulp build <out.ulp> <in1.txt> [in2.txt ...] [--delete-raw]");
                std::process::exit(2);
            }
            let out = args[2].clone();
            let inputs = args[3..].to_vec();
            let delete_raw = delete_raw_flag(&inputs);
            let inputs: Vec<String> = inputs.into_iter().filter(|a| a != "--delete-raw").collect();
            ok_or_exit(ulp::build(&inputs, &out, delete_raw), "build");
        }
        "append" => {
            if args.len() < 4 {
                eprintln!("usage: ulp append <db.ulp> <new1.txt> [new2.txt ...] [--delete-raw]");
                std::process::exit(2);
            }
            let db = args[2].clone();
            let inputs = args[3..].to_vec();
            let delete_raw = delete_raw_flag(&inputs);
            let inputs: Vec<String> = inputs.into_iter().filter(|a| a != "--delete-raw").collect();
            ok_or_exit(ulp::append(&db, &inputs, delete_raw), "append");
        }
        "repair" => {
            if args.len() < 3 {
                eprintln!("usage: ulp repair <file.ulp>");
                std::process::exit(2);
            }
            ok_or_exit(ulp::repair(&args[2]).map(|_| ()), "repair");
        }
        "merge" => {
            if args.len() < 4 {
                eprintln!("usage: ulp merge <in.ulp> <out.ulp>");
                std::process::exit(2);
            }
            ok_or_exit(ulp::merge(&args[2], &args[3]), "merge");
        }
        "sortcount" => {
            if args.len() < 3 {
                eprintln!("usage: ulp sortcount <in.ulp>");
                std::process::exit(2);
            }
            ok_or_exit(ulp::sortcount(&args[2]), "sortcount");
        }
        "query" => {
            if args.len() < 5 {
                eprintln!("usage: ulp query <file.ulp> <url|user|pass> <value> [-o out.txt]");
                std::process::exit(2);
            }
            let out = output_flag(&args[5..]);
            ok_or_exit(ulp::validate(&args[2]), "open ulp");
            let mut lock = open_output_or_exit(out.as_deref());
            ok_or_exit(
                ulp::query(&args[2], &args[3], &args[4], lock.as_mut()),
                "query",
            );
        }
        "dump" => {
            if args.len() < 3 {
                eprintln!("usage: ulp dump <file.ulp> [--frames] [-o out.txt]");
                std::process::exit(2);
            }
            let out = output_flag(&args[3..]);
            let frames = frames_flag(&args[3..]);
            ok_or_exit(ulp::validate(&args[2]), "open ulp");
            let mut lock = open_output_or_exit(out.as_deref());
            if frames {
                ok_or_exit(ulp::dump_frames(&args[2], lock.as_mut()), "dump");
            } else {
                ok_or_exit(ulp::dump(&args[2], lock.as_mut()), "dump");
            }
        }
        "info" => {
            if args.len() < 3 {
                eprintln!("usage: ulp info <file.ulp> [--json]");
                std::process::exit(2);
            }

            let (file, json) = if args[2] == "--json" && args.len() >= 4 {
                (&args[3], true)
            } else {
                (&args[2], args[3..].iter().any(|a| a == "--json"))
            };
            if json {
                ok_or_exit(ulp::info_json(file), "info");
            } else {
                ok_or_exit(ulp::info(file), "info");
            }
        }
        "bench" => {
            if args.len() < 6 {
                eprintln!("usage: ulp bench <file.ulp> <url|user|pass> <value> <N>");
                std::process::exit(2);
            }
            let Ok(n) = args[5].parse::<u64>() else {
                eprintln!("bench N must be a positive integer");
                std::process::exit(2);
            };
            if n == 0 {
                eprintln!("bench N must be greater than zero");
                std::process::exit(2);
            }
            ok_or_exit(ulp::bench(&args[2], &args[3], &args[4], n), "bench");
        }
        "match" => {
            if args.len() < 5 {
                eprintln!("usage: ulp match <file.ulp> <contains|prefix> <keyword> [-o out.txt]");
                eprintln!(
                    "       ulp match <file.ulp> user <prefix|suffix> <keyword> [-o out.txt]"
                );
                std::process::exit(2);
            }
            if args[3] == "user" {
                if args.len() < 6 {
                    eprintln!(
                        "usage: ulp match <file.ulp> user <prefix|suffix> <keyword> [-o out.txt]"
                    );
                    std::process::exit(2);
                }
                let out = output_flag(&args[6..]);
                ok_or_exit(ulp::validate(&args[2]), "open ulp");
                let mut lock = open_output_or_exit(out.as_deref());
                ok_or_exit(
                    ulp::match_user(&args[2], &args[4], &args[5], lock.as_mut()),
                    "match",
                );
            } else {
                let out = output_flag(&args[5..]);
                let frames = frames_flag(&args[5..]);
                ok_or_exit(ulp::validate(&args[2]), "open ulp");
                let mut lock = open_output_or_exit(out.as_deref());
                if frames {
                    ok_or_exit(
                        ulp::match_url_frames(&args[2], &args[3], &args[4], lock.as_mut()),
                        "match",
                    );
                } else {
                    ok_or_exit(
                        ulp::match_url(&args[2], &args[3], &args[4], lock.as_mut()),
                        "match",
                    );
                }
            }
        }
        "norm" => {
            if args.len() < 3 {
                eprintln!("usage: ulp norm <in1.txt> [in2.txt ...]");
                std::process::exit(2);
            }
            ok_or_exit(ulp::norm(&args[2..]), "norm");
        }
        "guide" => {
            if args.len() < 4 {
                eprintln!("usage: ulp guide <out.ulp> <folder-or-files...> [--delete-raw]");
                std::process::exit(2);
            }
            let out = args[2].clone();
            let inputs = args[3..].to_vec();
            let delete_raw = delete_raw_flag(&inputs);
            let inputs: Vec<String> = inputs.into_iter().filter(|a| a != "--delete-raw").collect();
            let mut guide_args = vec![out];
            guide_args.extend(inputs);
            ok_or_exit(ulp::guide(&guide_args, delete_raw), "guide");
        }
        _ => {
            eprintln!("unknown subcommand");
            std::process::exit(2);
        }
    }
}
