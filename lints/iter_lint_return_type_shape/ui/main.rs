// edition:2024

fn nested_ok() -> Result<Result<u8, String>, String> {
    Ok(Ok(1))
}

fn nested_err() -> Result<u8, Result<String, String>> {
    Ok(1)
}

fn option_result() -> Option<Result<u8, String>> {
    Some(Ok(1))
}

struct Worker;

impl Worker {
    fn method(&self) -> Option<Result<(), String>> {
        Some(Ok(()))
    }
}

fn accepted() -> Result<Option<u8>, String> {
    Ok(Some(1))
}

fn main() {
    let _ = accepted();
}
