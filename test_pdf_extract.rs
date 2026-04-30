use pdf_extract::extract_text;
use regex::Regex;
use encoding_rs::GB18030;

fn main() {
    let text = extract_text("C:\\Users\\Administrator\\workspace\\gaokaoapply\\上海市2025年普通高校招生本科普通批次平行志愿院校专业组投档分数线.pdf")
        .expect("extract");

    // Try UTF-8 first
    let text_str = match std::str::from_utf8(&text.into_bytes()) {
        Ok(s) => s,
        Err(_) => {
            let (cow, _, _) = GB18030.decode(&text.into_bytes());
            &cow
        }
    };

    // Print first 3000 chars
    let preview = &text_str[..text_str.len().min(3000)];
    println!("{}", preview);
    println!("---END PREVIEW---");

    // Try regex matching
    let re_line = Regex::new(r"^(\d{5,})\s+([^\d]+?)\s+(\d+分?及以上|\d+)$").unwrap();
    let mut count = 0;
    for line in text_str.lines() {
        if let Some(caps) = re_line.captures(line) {
            println!("MATCH: code={}, name={}, score={}",
                caps.get(1).unwrap().as_str(),
                caps.get(2).unwrap().as_str(),
                caps.get(3).unwrap().as_str());
            count += 1;
            if count >= 10 { break; }
        }
    }
    println!("Total matches found: {}", count);
}
