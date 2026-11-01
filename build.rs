//! Embed `src/assets/app_icon.ico` into the Windows PE executable (`RT_ICON` + `RT_GROUP_ICON`)
//! by emitting a standard COFF/Win32 `.res` file in `OUT_DIR` without requiring external `rc.exe`.

fn main() {
    println!("cargo:rerun-if-changed=src/assets/app_icon.ico");
    println!("cargo:rerun-if-changed=src/assets/app_icon.png");

    #[cfg(target_os = "windows")]
    {
        if let Err(e) = embed_windows_icon() {
            println!("cargo:warning=Could not embed Windows icon resource: {e}");
        }
    }
}

#[cfg(target_os = "windows")]
fn embed_windows_icon() -> Result<(), Box<dyn std::error::Error>> {
    use std::path::PathBuf;

    let ico = std::fs::read("src/assets/app_icon.ico")?;
    if ico.len() < 6
        || u16::from_le_bytes([ico[0], ico[1]]) != 0
        || u16::from_le_bytes([ico[2], ico[3]]) != 1
    {
        return Err("Invalid ICO header".into());
    }
    let count = u16::from_le_bytes([ico[4], ico[5]]) as usize;
    if ico.len() < 6 + count * 16 {
        return Err("Truncated ICO directory".into());
    }

    let mut res = Vec::with_capacity(ico.len() + 512);

    // 1. 32-byte null resource header required at the start of every 32-bit .res file
    res.extend_from_slice(&0u32.to_le_bytes()); // DataSize = 0
    res.extend_from_slice(&32u32.to_le_bytes()); // HeaderSize = 32
    res.extend_from_slice(&[0xFF, 0xFF, 0x00, 0x00]); // Type = 0
    res.extend_from_slice(&[0xFF, 0xFF, 0x00, 0x00]); // Name = 0
    res.extend_from_slice(&[0u8; 16]);

    // 2. Emit each RT_ICON (type = 3) entry with ordinal 1..=count
    let mut grp_dir = Vec::with_capacity(6 + count * 14);
    grp_dir.extend_from_slice(&ico[0..6]); // idReserved, idType, idCount

    for i in 0..count {
        let entry_off = 6 + i * 16;
        let entry = &ico[entry_off..entry_off + 16];
        let bytes_in_res = u32::from_le_bytes([entry[8], entry[9], entry[10], entry[11]]) as usize;
        let img_off = u32::from_le_bytes([entry[12], entry[13], entry[14], entry[15]]) as usize;
        if img_off + bytes_in_res > ico.len() {
            return Err("ICO image slice out of bounds".into());
        }
        let icon_id = (i + 1) as u16;

        // Append GRPICONDIRENTRY (12 bytes from ICONDIRENTRY + u16 icon_id)
        grp_dir.extend_from_slice(&entry[0..12]);
        grp_dir.extend_from_slice(&icon_id.to_le_bytes());

        // Write RT_ICON resource header (32 bytes)
        write_res_header(&mut res, bytes_in_res as u32, 3, icon_id, 0x1010);
        res.extend_from_slice(&ico[img_off..img_off + bytes_in_res]);
        align4(&mut res);
    }

    // 3. Emit RT_GROUP_ICON (type = 14) with ordinal 1 ("MAINICON")
    write_res_header(&mut res, grp_dir.len() as u32, 14, 1, 0x1030);
    res.extend_from_slice(&grp_dir);
    align4(&mut res);

    let out_dir = PathBuf::from(std::env::var("OUT_DIR")?);
    let res_path = out_dir.join("app_icon.res");
    std::fs::write(&res_path, &res)?;

    // GNU ld cannot read a raw .res; windres (MinGW binutils) converts it to a COFF object.
    let link_path = if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("gnu") {
        let obj_path = out_dir.join("app_icon.o");
        let ok = std::process::Command::new("windres")
            .args(["-J", "res", "-O", "coff", "-i"])
            .arg(&res_path)
            .arg("-o")
            .arg(&obj_path)
            .status()?
            .success();
        if !ok {
            return Err("windres failed".into());
        }
        obj_path
    } else {
        res_path
    };

    println!("cargo:rustc-link-arg-bin=ctdozlite={}", link_path.display());
    Ok(())
}

#[cfg(target_os = "windows")]
fn write_res_header(
    buf: &mut Vec<u8>,
    data_size: u32,
    res_type: u16,
    res_name: u16,
    mem_flags: u16,
) {
    buf.extend_from_slice(&data_size.to_le_bytes());
    buf.extend_from_slice(&32u32.to_le_bytes()); // HeaderSize = 32
    buf.extend_from_slice(&0xFFFFu16.to_le_bytes());
    buf.extend_from_slice(&res_type.to_le_bytes());
    buf.extend_from_slice(&0xFFFFu16.to_le_bytes());
    buf.extend_from_slice(&res_name.to_le_bytes());
    buf.extend_from_slice(&0u32.to_le_bytes()); // DataVersion
    buf.extend_from_slice(&mem_flags.to_le_bytes());
    buf.extend_from_slice(&0x0409u16.to_le_bytes()); // LanguageId (en-US)
    buf.extend_from_slice(&0u32.to_le_bytes()); // Version
    buf.extend_from_slice(&0u32.to_le_bytes()); // Characteristics
}

#[cfg(target_os = "windows")]
fn align4(buf: &mut Vec<u8>) {
    while buf.len() % 4 != 0 {
        buf.push(0);
    }
}
