fn main() {
    use pdf_extract::extract_text;
    let text = extract_text("上海市2025年普通高校招生本科普通批次平行志愿院校专业组投档分数线.pdf")
        .expect("extract");
    // Print first 2000 chars
    println!("{}", &text[..text.len().min(2000)]);
}
