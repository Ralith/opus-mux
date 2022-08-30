use magnum_opus::{Channels, Decoder};

fn main() {
    let args = std::env::args().collect::<Vec<_>>();
    if args.len() != 3 {
        println!("Usage: {} <input.opus> <output.wav>", args[0]);
        return;
    }
    let data = std::fs::read(&args[1]).expect("couldn't read file");
    let mut demuxer = opus_mux::Demuxer::new();
    demuxer.push(&data).unwrap();
    let header = *demuxer.header().unwrap();
    println!("{:?}", header);

    let channels = match header.channels {
        1 => Channels::Mono,
        2 => Channels::Stereo,
        _ => panic!("unsupported channel count"),
    };

    let mut decoder = Decoder::new(48000, channels).unwrap();
    decoder.set_gain(header.output_gain.into()).unwrap();
    let mut writer = hound::WavWriter::create(
        &args[2],
        hound::WavSpec {
            channels: header.channels.into(),
            sample_rate: 48000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        },
    )
    .unwrap();
    let mut total = 0;
    let mut buf = Vec::new();
    let mut skip = header.pre_skip;
    while let Some(packet) = demuxer.next() {
        let n = decoder.get_nb_samples(packet).unwrap();
        total += n;
        let samples = n * usize::from(header.channels);
        buf.resize(samples, 0);
        decoder.decode(packet, &mut buf, false).unwrap();
        let skipped = buf.len().min(skip as usize) as u16;
        buf.drain(..skipped as usize);
        skip -= skipped;
        let mut writer = writer.get_i16_writer(buf.len() as u32);
        for &x in &buf {
            writer.write_sample(x);
        }
        writer.flush().unwrap();
    }
    writer.finalize().unwrap();

    println!("decoded {} frames", total);
}
