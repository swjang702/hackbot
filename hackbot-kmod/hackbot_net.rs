// SPDX-License-Identifier: GPL-2.0

//! Kernel socket wrapper, HTTP helpers, JSON parsing, and format_usize.

use core::mem::MaybeUninit;
use core::ptr;

use kernel::{bindings, prelude::*};

use crate::config::*;

// ---------------------------------------------------------------------------
// Kernel socket wrapper
// ---------------------------------------------------------------------------

/// IPv4 socket address.
#[repr(C)]
pub(crate) struct SockaddrIn {
    sin_family: u16,
    sin_port: u16,
    sin_addr: u32,
    __pad: [u8; 8],
}

/// RAII wrapper around a kernel socket.
pub(crate) struct KernelSocket {
    sock: *mut bindings::socket,
}

impl KernelSocket {
    pub(crate) fn connect_tcp(addr: u32, port: u16) -> Result<Self> {
        let mut sock: *mut bindings::socket = ptr::null_mut();

        let ret = unsafe {
            bindings::sock_create_kern(
                ptr::addr_of_mut!(bindings::init_net),
                bindings::AF_INET as i32,
                bindings::sock_type_SOCK_STREAM as i32,
                IPPROTO_TCP,
                &mut sock,
            )
        };
        if ret < 0 {
            pr_err!("hackbot: sock_create_kern failed: {}\n", ret);
            return Err(Error::from_errno(ret));
        }

        let socket = Self { sock };

        let addr_in = SockaddrIn {
            sin_family: bindings::AF_INET as u16,
            sin_port: port.to_be(),
            sin_addr: addr.to_be(),
            __pad: [0u8; 8],
        };

        let ret = unsafe {
            bindings::kernel_connect(
                socket.sock,
                &addr_in as *const SockaddrIn as *mut bindings::sockaddr_unsized,
                core::mem::size_of::<SockaddrIn>() as i32,
                0,
            )
        };
        if ret < 0 {
            pr_err!("hackbot: kernel_connect failed: {}\n", ret);
            return Err(Error::from_errno(ret));
        }

        Ok(socket)
    }

    pub(crate) fn send_all(&self, buf: &[u8]) -> Result<()> {
        let mut sent = 0usize;

        while sent < buf.len() {
            let remaining = &buf[sent..];
            let mut kv = bindings::kvec {
                iov_base: remaining.as_ptr() as *mut core::ffi::c_void,
                iov_len: remaining.len(),
            };

            let mut msg: bindings::msghdr = unsafe { MaybeUninit::zeroed().assume_init() };

            let ret = unsafe {
                bindings::kernel_sendmsg(self.sock, &mut msg, &mut kv, 1, remaining.len())
            };

            if ret < 0 {
                return Err(Error::from_errno(ret));
            }
            if ret == 0 {
                return Err(EPIPE);
            }
            sent += ret as usize;
        }

        Ok(())
    }

    pub(crate) fn recv(&self, buf: &mut [u8]) -> Result<usize> {
        let mut kv = bindings::kvec {
            iov_base: buf.as_mut_ptr() as *mut core::ffi::c_void,
            iov_len: buf.len(),
        };

        let mut msg: bindings::msghdr = unsafe { MaybeUninit::zeroed().assume_init() };

        let ret = unsafe {
            bindings::kernel_recvmsg(self.sock, &mut msg, &mut kv, 1, buf.len(), 0)
        };

        if ret < 0 {
            return Err(Error::from_errno(ret));
        }
        Ok(ret as usize)
    }

    pub(crate) fn recv_all(&self, response: &mut KVVec<u8>, max_size: usize) -> Result<()> {
        let mut tmp = [0u8; RECV_BUF_SIZE];

        loop {
            if response.len() >= max_size {
                pr_warn!("hackbot: response truncated at {} bytes\n", max_size);
                break;
            }

            let n = self.recv(&mut tmp)?;
            if n == 0 {
                break;
            }

            let _ = response.extend_from_slice(&tmp[..n], GFP_KERNEL);
        }

        Ok(())
    }
}

impl Drop for KernelSocket {
    fn drop(&mut self) {
        unsafe { bindings::sock_release(self.sock) };
    }
}

// ---------------------------------------------------------------------------
// HTTP helpers
// ---------------------------------------------------------------------------

/// Append a dotted-decimal IPv4 address to a KVVec.
pub(crate) fn append_ipv4(buf: &mut KVVec<u8>, addr: u32) {
    let octets = [
        ((addr >> 24) & 0xFF) as u8,
        ((addr >> 16) & 0xFF) as u8,
        ((addr >> 8) & 0xFF) as u8,
        (addr & 0xFF) as u8,
    ];
    for (i, &octet) in octets.iter().enumerate() {
        if i > 0 {
            let _ = buf.push(b'.', GFP_KERNEL);
        }
        let mut num_buf = [0u8; 20];
        let s = format_usize(octet as usize, &mut num_buf);
        let _ = buf.extend_from_slice(s, GFP_KERNEL);
    }
}

/// Escape a byte string for JSON.
pub(crate) fn json_escape(input: &[u8], output: &mut KVVec<u8>) {
    for &b in input {
        match b {
            b'\\' => { let _ = output.extend_from_slice(b"\\\\", GFP_KERNEL); }
            b'"'  => { let _ = output.extend_from_slice(b"\\\"", GFP_KERNEL); }
            b'\n' => { let _ = output.extend_from_slice(b"\\n", GFP_KERNEL); }
            b'\r' => { let _ = output.extend_from_slice(b"\\r", GFP_KERNEL); }
            b'\t' => { let _ = output.extend_from_slice(b"\\t", GFP_KERNEL); }
            c if c < 0x20 => {}
            _ => { let _ = output.push(b, GFP_KERNEL); }
        }
    }
}

/// Append a chat message to a JSON messages array.
pub(crate) fn append_message_to_json(messages: &mut KVVec<u8>, role: &[u8], content: &[u8]) {
    if messages.last() == Some(&b']') {
        messages.truncate(messages.len() - 1);
        let _ = messages.extend_from_slice(b",", GFP_KERNEL);
    } else {
        let _ = messages.extend_from_slice(b"[", GFP_KERNEL);
    }
    let _ = messages.extend_from_slice(b"{\"role\":\"", GFP_KERNEL);
    let _ = messages.extend_from_slice(role, GFP_KERNEL);
    let _ = messages.extend_from_slice(b"\",\"content\":\"", GFP_KERNEL);
    json_escape(content, messages);
    let _ = messages.extend_from_slice(b"\"}]", GFP_KERNEL);
}

/// Find the HTTP response body.
pub(crate) fn find_http_body(raw: &[u8]) -> &[u8] {
    if raw.len() < 4 {
        return raw;
    }
    for i in 0..raw.len() - 3 {
        if &raw[i..i + 4] == b"\r\n\r\n" {
            return &raw[i + 4..];
        }
    }
    raw
}

/// Extract the HTTP status code.
pub(crate) fn parse_http_status(raw: &[u8]) -> u16 {
    let prefix = b"HTTP/1.";
    if raw.len() < 12 || &raw[..7] != prefix {
        return 0;
    }
    if raw[8] != b' ' {
        return 0;
    }
    let d0 = raw[9].wrapping_sub(b'0') as u16;
    let d1 = raw[10].wrapping_sub(b'0') as u16;
    let d2 = raw[11].wrapping_sub(b'0') as u16;
    if d0 > 9 || d1 > 9 || d2 > 9 {
        return 0;
    }
    d0 * 100 + d1 * 10 + d2
}

// ---------------------------------------------------------------------------
// JSON parsing
// ---------------------------------------------------------------------------

/// Extract the "text"/"content" field value from a vLLM JSON response.
pub(crate) fn extract_text_from_json<'a>(json: &'a [u8]) -> Option<&'a [u8]> {
    let patterns: &[&[u8]] = &[
        b"\"content\":\"", b"\"content\": \"",
        b"\"text\":\"", b"\"text\": \"",
    ];

    let (start_pos, pat_len) = patterns.iter().find_map(|pat| {
        find_subsequence(json, pat).map(|pos| (pos, pat.len()))
    })?;

    let value_start = start_pos + pat_len;
    let value_end = find_json_string_end(json, value_start)?;

    Some(&json[value_start..value_end])
}

/// Find the position of a subsequence in a byte slice.
pub(crate) fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    for i in 0..=haystack.len() - needle.len() {
        if &haystack[i..i + needle.len()] == needle {
            return Some(i);
        }
    }
    None
}

/// Find the end of a JSON string value.
pub(crate) fn find_json_string_end(json: &[u8], start: usize) -> Option<usize> {
    let mut i = start;
    while i < json.len() {
        match json[i] {
            b'"' => return Some(i),
            b'\\' => i = i.saturating_add(2),
            _ => i += 1,
        }
    }
    None
}

/// Unescape a JSON string value.
pub(crate) fn json_unescape(escaped: &[u8], output: &mut KVVec<u8>) {
    let mut i = 0;
    while i < escaped.len() {
        if escaped[i] == b'\\' && i + 1 < escaped.len() {
            let c = match escaped[i + 1] {
                b'n' => b'\n',
                b'r' => b'\r',
                b't' => b'\t',
                b'\\' => b'\\',
                b'"' => b'"',
                b'/' => b'/',
                other => {
                    let _ = output.push(b'\\', GFP_KERNEL);
                    let _ = output.push(other, GFP_KERNEL);
                    i += 2;
                    continue;
                }
            };
            let _ = output.push(c, GFP_KERNEL);
            i += 2;
        } else {
            let _ = output.push(escaped[i], GFP_KERNEL);
            i += 1;
        }
    }
}

/// Format a usize as decimal ASCII. No heap allocation.
pub(crate) fn format_usize(mut n: usize, buf: &mut [u8; 20]) -> &[u8] {
    if n == 0 {
        buf[0] = b'0';
        return &buf[..1];
    }
    let mut pos = 20;
    while n > 0 {
        pos -= 1;
        buf[pos] = b'0' + (n % 10) as u8;
        n /= 10;
    }
    &buf[pos..]
}
