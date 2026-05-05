//! Entry point — routage entre mode GUI et mode worker (--worker).
//!
//! Référence Python : `python/a3000_transfer/__main__.py`

fn main() -> anyhow::Result<()> {
    // TODO Phase 3 :
    // - Parser argv pour détecter --worker → lance worker process
    // - Sinon : init logging tracing + lance GUI eframe
    // - UAC élévation via ShellExecuteExW si transfert SCSI déclenché

    println!("a3000-transfer — placeholder. TODO Phase 3.");
    Ok(())
}
