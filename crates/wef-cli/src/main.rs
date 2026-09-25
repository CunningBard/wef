fn main() {
    match wef_cli::run_with_args(std::env::args().skip(1)) {
        Ok(output) if output.is_empty() => {}
        Ok(output) => println!("{output}"),
        Err(error) => {
            eprintln!("error: {error}");
            std::process::exit(1);
        }
    }
}
