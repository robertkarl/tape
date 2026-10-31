//! WEB UI — owned by T5. axum, bound to the LAN address only (decision D3):
//! no nginx, no TLS, no auth, no DNS. Read-only; no handler may write.

pub fn serve(addr: &str, port: u16) -> anyhow::Result<()> {
    let _ = (addr, port);
    anyhow::bail!("not implemented (T5): list + /s/{host}/{id} session pages")
}
