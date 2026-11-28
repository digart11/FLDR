//! Offline notices remain available even if the website is unreachable.
pub(crate) const NOTICES_URL: &str = "atlas:third-party-notices";

pub(crate) fn open_notices() {
    // One-shot disk I/O and launching the text viewer stay off the GPUI thread.
    std::thread::spawn(|| {
        let directory = atlas_core::util::config::data_dir().join("legal");
        let result = (|| -> anyhow::Result<()> {
            std::fs::create_dir_all(&directory)?;
            std::fs::write(directory.join("LICENSE.txt"), include_str!("../../../../../LICENSE"))?;
            std::fs::write(directory.join("NOTICE.txt"), include_str!("../../../../../NOTICE"))?;
            std::fs::write(directory.join("SOURCE-AVAILABILITY.txt"), include_str!("../../../../../docs/fileop-source-availability.txt"))?;
            let path = directory.join("THIRD-PARTY-NOTICES.txt");
            std::fs::write(&path, concat!(
                include_str!("../../../../../NOTICE"), "\n\n",
                include_str!("../../../../../docs/fileop-source-availability.txt"), "\n\n",
                include_str!("../../../../../LICENSE"), "\n\n",
                include_str!("../../../../../legal/THIRD-PARTY-NOTICES.txt")
            ))?;
            open::that(&path)?;
            Ok(())
        })();
        if let Err(error) = result {
            tracing::error!(%error, "could not open third-party notices");
        }
    });
}
