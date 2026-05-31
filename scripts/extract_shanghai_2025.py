import pdfplumber
import re
import json

# Characters that only appear as watermark single-chars (the phrase "上海市教育考试院" scattered)
WATERMARK = set("院试考育教市海上")


def strip_watermark(s):
    """Remove watermark characters that are truly isolated.
    Isolated = not adjacent to ANY CJK character (including other watermark chars).
    e.g. '考\n580分及以上' -> '580分及以上'
         '上海交大(01)'  -> '上海交大(01)'  (上/海 adjacent to each other or 交, kept)
    """
    if not s:
        return s
    chars = list(s)
    result = []
    for i, c in enumerate(chars):
        if c not in WATERMARK:
            result.append(c)
            continue
        prev = chars[i - 1] if i > 0 else ""
        nxt  = chars[i + 1] if i < len(chars) - 1 else ""
        # Keep if adjacent to ANY CJK character (real word context)
        prev_cjk = "\u4e00" <= prev <= "\u9fff"
        nxt_cjk  = "\u4e00" <= nxt  <= "\u9fff"
        if prev_cjk or nxt_cjk:
            result.append(c)
        # else: truly isolated watermark char, drop it
    return "".join(result)


def parse_cutoff(raw):
    """Parse cutoff cell text into int or '580分及以上' string."""
    raw = re.sub(r"\s+", "", raw or "")  # collapse whitespace first
    has_above = "以上" in raw          # check BEFORE stripping
    cleaned = strip_watermark(raw)
    if has_above:
        m = re.search(r"(\d{3})", cleaned)
        num = int(m.group(1)) if m else 580
        return f"{num}分及以上"
    m = re.match(r"^(\d{3})$", cleaned)
    if m:
        v = int(m.group(1))
        if 300 <= v <= 700:
            return v
    return None


path = r"E:\projects\gaokaoapply\上海市2025年普通高校招生本科普通批次平行志愿院校专业组投档分数线.pdf"

entries = []
seen_codes = set()

with pdfplumber.open(path) as pdf:
    for pg in pdf.pages:
        tables = pg.extract_tables()
        for table in tables:
            for row in table:
                if not row or len(row) < 3:
                    continue
                code_raw = strip_watermark(row[0] or "").strip()
                name_raw = strip_watermark(row[1] or "").strip()
                cutoff_raw = row[2] or ""

                # Code must be exactly 5 digits
                if not re.match(r"^\d{5}$", code_raw):
                    continue
                if code_raw in seen_codes:
                    continue
                # Name must be non-empty after stripping
                name = re.sub(r"\s+", "", name_raw)
                if not name:
                    continue

                cutoff = parse_cutoff(cutoff_raw)
                if cutoff is None:
                    continue

                seen_codes.add(code_raw)
                entries.append({"code": code_raw, "name": name, "cutoff": cutoff})

result = {
    "title": "2025年上海市普通高校招生本科普通批次平行志愿院校专业组投档分数线",
    "year": 2025,
    "province": "上海",
    "note": "580分及以上考生投档信息另行告知；部分院校Q组、中外合作办学院校专业组投档结果另行公布",
    "data": entries,
}

out = r"E:\projects\gaokaoapply\src\data\gaokao\shanghai_2025_cutoffs.json"
with open(out, "w", encoding="utf-8") as f:
    json.dump(result, f, ensure_ascii=False, indent=2)

print(f"写入 {len(entries)} 条记录 -> {out}")
top = [e for e in entries if isinstance(e["cutoff"], str)]
print(f"580分及以上: {len(top)} 条，示例: {[e['name'] for e in top[:5]]}")
normal = [e for e in entries if isinstance(e["cutoff"], int)]
print(f"普通分数线: {len(normal)} 条，示例:")
for e in normal[:5]:
    print(f"  {e['code']} {e['name']} {e['cutoff']}")
