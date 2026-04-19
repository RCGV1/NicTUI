use csv::Reader;
use std::collections::HashMap;

#[derive(Debug, Default)]
struct Channel {
    power: u8,
}

fn main() {
    let data = "Channel_Num,Name,RX_Freq,TX_Power\n1,Test,144.0, 0";
    let mut rdr = Reader::from_reader(data.as_bytes());

    for result in rdr.deserialize::<HashMap<String, String>>() {
        let row = result.unwrap();

        let get_val = |key: &str| -> Option<&String> {
            row.get(key).or_else(|| {
                let key_lower = key.to_lowercase();
                row.iter()
                    .find(|(k, _)| k.to_lowercase() == key_lower)
                    .map(|(_, v)| v)
            })
        };

        let power = get_val("TX_Power")
            .and_then(|s| s.parse::<u8>().ok())
            .unwrap_or(0);
        let channel = Channel { power };

        println!("Parsed Power: {}", channel.power);
    }
}
