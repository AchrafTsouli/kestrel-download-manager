//! The per-user named pipe between `flux-nmhost` and Flux. Only the current Windows user (and SYSTEM)
//! may open it, and the client checks that the server runs as the same user, so another account
//! cannot stand in for Flux by creating the pipe first.

use std::ffi::c_void;
use std::time::Duration;

use sha2::{Digest, Sha256};
use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeClient, NamedPipeServer, ServerOptions};
use windows_sys::Win32::Foundation::{CloseHandle, ERROR_PIPE_BUSY, HANDLE, HLOCAL, LocalFree};
use windows_sys::Win32::Security::Authorization::{ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1};
use windows_sys::Win32::Security::{GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser};
use windows_sys::Win32::System::Pipes::GetNamedPipeServerProcessId;
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION};

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

/// The string SID of the user a process runs as.
fn process_user(process: HANDLE) -> Option<String> {
    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(process, TOKEN_QUERY, &mut token) == 0 {
            return None;
        }
        let mut needed = 0u32;
        GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut needed);
        let mut buffer = vec![0u8; needed as usize];
        let ok = GetTokenInformation(token, TokenUser, buffer.as_mut_ptr().cast(), needed, &mut needed);
        CloseHandle(token);
        if ok == 0 {
            return None;
        }
        let user = &*(buffer.as_ptr() as *const TOKEN_USER);
        let mut text: *mut u16 = std::ptr::null_mut();
        if ConvertSidToStringSidW(user.User.Sid, &mut text) == 0 {
            return None;
        }
        let len = (0..).take_while(|&i| *text.add(i) != 0).count();
        let sid = String::from_utf16_lossy(std::slice::from_raw_parts(text, len));
        LocalFree(text as HLOCAL);
        Some(sid)
    }
}

pub fn current_user() -> String {
    unsafe { process_user(GetCurrentProcess()) }.unwrap_or_else(|| "unknown".into())
}

/// `\\.\pipe\flux-bridge-<hash of the user>`: one pipe per Windows user.
pub fn pipe_name() -> String {
    let hash = Sha256::digest(format!("flux-bridge|{}", current_user()).as_bytes());
    format!(r"\\.\pipe\flux-bridge-{}", hash.iter().take(12).map(|b| format!("{b:02x}")).collect::<String>())
}

/// A pipe instance only this user (and SYSTEM) can open. `first` fails if someone already owns the name.
pub fn create_server(first: bool) -> std::io::Result<NamedPipeServer> {
    let sddl = wide(&format!("D:P(A;;GA;;;{})(A;;GA;;;SY)", current_user()));
    let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    if unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(), SDDL_REVISION_1, &mut descriptor, std::ptr::null_mut()) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    let mut attributes = SECURITY_ATTRIBUTES { nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32, lpSecurityDescriptor: descriptor, bInheritHandle: 0 };
    let result = unsafe {
        ServerOptions::new().first_pipe_instance(first).reject_remote_clients(true).create_with_security_attributes_raw(pipe_name(), &mut attributes as *mut _ as *mut c_void)
    };
    unsafe { LocalFree(descriptor as HLOCAL) };
    result
}

/// Connects to the running Flux, refusing a pipe served by another user's process.
pub async fn connect(timeout: Duration) -> std::io::Result<NamedPipeClient> {
    let deadline = tokio::time::Instant::now() + timeout;
    let client = loop {
        match ClientOptions::new().open(pipe_name()) {
            Ok(client) => break client,
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) || e.kind() == std::io::ErrorKind::NotFound => {
                if tokio::time::Instant::now() >= deadline {
                    return Err(e);
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Err(e) => return Err(e),
        }
    };
    use std::os::windows::io::AsRawHandle;
    let mut pid = 0u32;
    let same_user = unsafe {
        GetNamedPipeServerProcessId(client.as_raw_handle() as HANDLE, &mut pid) != 0 && {
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            let user = if process.is_null() { None } else { process_user(process) };
            if !process.is_null() { CloseHandle(process); }
            user.as_deref() == Some(current_user().as_str())
        }
    };
    if !same_user {
        return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "The Flux pipe is served by another user's program."));
    }
    Ok(client)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Request, Response, read_frame, write_frame};

    #[tokio::test]
    async fn a_request_crosses_the_pipe_and_a_second_owner_is_refused() {
        let mut server = create_server(true).expect("pipe");
        assert!(create_server(true).is_err(), "a second first instance must fail");
        let serve = tokio::spawn(async move {
            server.connect().await.unwrap();
            let request: Request = read_frame(&mut server).await.unwrap().unwrap();
            write_frame(&mut server, &Response { ok: request.action == "ping", ..Default::default() }).await.unwrap();
        });
        let mut client = connect(Duration::from_secs(2)).await.unwrap();
        write_frame(&mut client, &Request { action: "ping".into(), ..Default::default() }).await.unwrap();
        let response: Response = read_frame(&mut client).await.unwrap().unwrap();
        assert!(response.ok);
        serve.await.unwrap();
    }
}
