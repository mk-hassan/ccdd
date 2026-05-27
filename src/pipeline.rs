use std::error::Error;
use std::fmt::Display;
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::time::{Duration, Instant};

use crate::SourceType;

pub struct Metrics {
    total_bytes: usize,
    read_blocks: usize,
    read_partials: usize,
    write_blocks: usize,
    write_partials: usize,
    time_duration: Duration,
}

impl Display for Metrics {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let secs = self.time_duration.as_secs_f64();
        let mb_per_sec = (self.total_bytes as f64) / (1024.0 * 1024.0) / secs;
        writeln!(f, "{}+{} records in", self.read_blocks, self.read_partials)?;
        writeln!(
            f,
            "{}+{} records out",
            self.write_blocks, self.write_partials
        )?;
        write!(
            f,
            "{} bytes copied, {:.6} s, {:.2} MB/s",
            self.total_bytes, secs, mb_per_sec
        )
    }
}

pub fn run(
    source: &SourceType,
    destination: &SourceType,
    ibs: usize,
    obs: usize,
) -> Result<Metrics, Box<dyn Error>> {
    let start = Instant::now();
    let mut reader = open_read_buffer(source, ibs)?;
    let mut writer = open_write_buffer(destination, obs)?;

    let mut metrics = Metrics {
        total_bytes: 0,
        read_blocks: 0,
        read_partials: 0,
        write_blocks: 0,
        write_partials: 0,
        time_duration: Duration::new(0, 0),
    };

    let mut buffer: Vec<u8> = vec![0; ibs];
	let mut accum: Vec<u8> = Vec::new();

    while let Ok(reads) = reader.read(&mut buffer) {
        if reads == 0 { break; }

        if reads == ibs { metrics.read_blocks += 1; } 
		else { metrics.read_partials += 1; }
        metrics.total_bytes += reads;

		accum.extend_from_slice(&buffer[..reads]); // only valid bytes
        while accum.len() >= obs {
			writer.write_all(&accum[..obs])?;
            accum.drain(..obs);
            metrics.write_blocks += 1;
        }
    }

    if !accum.is_empty() {
        writer.write_all(&accum)?;
        metrics.write_partials += 1;
    }

    metrics.time_duration = start.elapsed();
    Ok(metrics)
}

fn open_read_buffer(
    source: &SourceType,
    capacity: usize,
) -> Result<Box<dyn BufRead>, Box<dyn Error>> {
    match source {
        SourceType::File(path) => Ok(Box::new(BufReader::with_capacity(
            capacity,
            File::open(path)?,
        ))),
        SourceType::Standard => Ok(Box::new(BufReader::with_capacity(
            capacity,
            std::io::stdin(),
        ))),
    }
}

fn open_write_buffer(
    destination: &SourceType,
    capacity: usize,
) -> Result<Box<dyn Write>, Box<dyn Error>> {
    match destination {
        SourceType::File(path) => Ok(Box::new(BufWriter::with_capacity(
            capacity,
            File::create(path)?,
        ))),
        SourceType::Standard => Ok(Box::new(BufWriter::with_capacity(
            capacity,
            std::io::stdout(),
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SourceType;
    use std::fs;

    /// Runs the pipeline, asserts output == input, returns Metrics.
    fn run_copy(test_name: &str, input: &[u8], ibs: usize, obs: usize) -> Metrics {
        let dir = std::env::temp_dir();
        let in_path = dir.join(format!("ccdd_{}_in.bin", test_name));
        let out_path: std::path::PathBuf = dir.join(format!("ccdd_{}_out.bin", test_name));

        fs::write(&in_path, input).unwrap();

        let metrics = run(
            &SourceType::File(in_path.to_str().unwrap().to_string()),
            &SourceType::File(out_path.to_str().unwrap().to_string()),
            ibs,
            obs,
        )
        .unwrap();

        let output = fs::read(&out_path).unwrap();
        assert_eq!(input, output.as_slice(), "[{}] content mismatch", test_name);

        let _ = fs::remove_file(&in_path);
        let _ = fs::remove_file(&out_path);
        metrics
    }

    // ── content correctness ──────────────────────────────────────────────────

    #[test]
    fn test_empty_file() {
        // read() returns Ok(0) immediately → no blocks, no writes
        let m = run_copy("empty", &[], 512, 512);
        assert_eq!(m.total_bytes,   0);
        assert_eq!(m.read_blocks,   0);
        assert_eq!(m.read_partials, 0);
        assert_eq!(m.write_blocks,  0);
        assert_eq!(m.write_partials,0);
    }

    #[test]
    fn test_single_byte() {
        // 1 byte < ibs=512 → partial read + partial write
        let m = run_copy("single_byte", &[0xAB], 512, 512);
        assert_eq!(m.total_bytes,    1);
        assert_eq!(m.read_blocks,    0);
        assert_eq!(m.read_partials,  1);
        assert_eq!(m.write_blocks,   0);
        assert_eq!(m.write_partials, 1);
    }

    #[test]
    fn test_all_256_byte_values_preserved() {
        // Every possible byte value survives the copy unchanged
        let input: Vec<u8> = (0u8..=255).collect();
        run_copy("all_bytes", &input, 64, 64);
    }

    // ── ibs == obs ───────────────────────────────────────────────────────────

    #[test]
    fn test_exact_multiple_of_block_size() {
        // 1024 bytes, ibs=obs=512 → 2 full reads, 2 full writes, no partials
        let input: Vec<u8> = (0..1024).map(|i: u16| (i % 256) as u8).collect();
        let m = run_copy("exact_mult", &input, 512, 512);
        assert_eq!(m.total_bytes,    1024);
        assert_eq!(m.read_blocks,    2);
        assert_eq!(m.read_partials,  0);
        assert_eq!(m.write_blocks,   2);
        assert_eq!(m.write_partials, 0);
    }

    #[test]
    fn test_not_multiple_of_block_size() {
        // 1000 bytes, ibs=obs=512 → 1 full + 1 partial on both sides
        let input: Vec<u8> = (0..1000).map(|i: u16| (i % 256) as u8).collect();
        let m = run_copy("not_mult", &input, 512, 512);
        assert_eq!(m.total_bytes,    1000);
        assert_eq!(m.read_blocks,    1);
        assert_eq!(m.read_partials,  1);
        assert_eq!(m.write_blocks,   1);
        assert_eq!(m.write_partials, 1);
    }

    #[test]
    fn test_file_size_exactly_ibs() {
        // File == ibs → 1 full read, 1 full write, 0 partials
        let input = vec![0xAA; 512];
        let m = run_copy("eq_ibs", &input, 512, 512);
        assert_eq!(m.total_bytes,    512);
        assert_eq!(m.read_blocks,    1);
        assert_eq!(m.read_partials,  0);
        assert_eq!(m.write_blocks,   1);
        assert_eq!(m.write_partials, 0);
    }

    #[test]
    fn test_file_smaller_than_ibs() {
        // 100 bytes, ibs=512 → single partial read + single partial write
        let input = vec![0xBB; 100];
        let m = run_copy("lt_ibs", &input, 512, 512);
        assert_eq!(m.total_bytes,    100);
        assert_eq!(m.read_blocks,    0);
        assert_eq!(m.read_partials,  1);
        assert_eq!(m.write_blocks,   0);
        assert_eq!(m.write_partials, 1);
    }

    // ── ibs < obs (accumulation) ─────────────────────────────────────────────

    #[test]
    fn test_ibs_less_than_obs() {
        // 1000 bytes, ibs=200, obs=512
        // 5 full reads of 200; after read3 accum=600 → write 512; end accum=488 → partial
        let input: Vec<u8> = (0..1000).map(|i: u16| (i % 256) as u8).collect();
        let m = run_copy("ibs_lt_obs", &input, 200, 512);
        assert_eq!(m.total_bytes,    1000);
        assert_eq!(m.read_blocks,    5);
        assert_eq!(m.read_partials,  0);
        assert_eq!(m.write_blocks,   1);
        assert_eq!(m.write_partials, 1);
    }

    #[test]
    fn test_obs_larger_than_whole_file() {
        // obs > file size → accum never reaches threshold → single partial write
        let input = vec![0xCC; 100];
        let m = run_copy("obs_gt_file", &input, 512, 1024);
        assert_eq!(m.total_bytes,    100);
        assert_eq!(m.write_blocks,   0);
        assert_eq!(m.write_partials, 1);
    }

    // ── ibs > obs (multiple writes per read) ────────────────────────────────

    #[test]
    fn test_ibs_greater_than_obs() {
        // 1000 bytes, ibs=512, obs=200
        // read1=512 → write 200+200, accum=112
        // read2=488 → accum=600 → write 200+200+200, accum=0
        // write_blocks=5, write_partials=0
        let input: Vec<u8> = (0..1000).map(|i: u16| (i % 256) as u8).collect();
        let m = run_copy("ibs_gt_obs", &input, 512, 200);
        assert_eq!(m.total_bytes,    1000);
        assert_eq!(m.read_blocks,    1);
        assert_eq!(m.read_partials,  1);
        assert_eq!(m.write_blocks,   5);
        assert_eq!(m.write_partials, 0);
    }

    // ── extreme block sizes ──────────────────────────────────────────────────

    #[test]
    fn test_byte_by_byte() {
        // ibs=1, obs=1 → every byte is a full read block and full write block
        let m = run_copy("byte_by_byte", b"hello", 1, 1);
        assert_eq!(m.total_bytes,    5);
        assert_eq!(m.read_blocks,    5);
        assert_eq!(m.read_partials,  0);
        assert_eq!(m.write_blocks,   5);
        assert_eq!(m.write_partials, 0);
    }

    #[test]
    fn test_ibs_1_obs_larger_than_file() {
        // ibs=1, obs=100, file=50 bytes
        // 50 full read blocks (ibs=1); accum never reaches 100 → single partial write
        let input = vec![0x42; 50];
        let m = run_copy("ibs1_obs100", &input, 1, 100);
        assert_eq!(m.total_bytes,    50);
        assert_eq!(m.read_blocks,    50);
        assert_eq!(m.read_partials,  0);
        assert_eq!(m.write_blocks,   0);
        assert_eq!(m.write_partials, 1);
    }

    #[test]
    fn test_large_file_content_integrity() {
        // 1 MB, ibs=4096, obs=4096 — no byte lost or corrupted
        let input: Vec<u8> = (0..1024 * 1024).map(|i: u32| (i % 256) as u8).collect();
        let m = run_copy("large_1mb", &input, 4096, 4096);
        assert_eq!(m.total_bytes, 1024 * 1024);
    }
}