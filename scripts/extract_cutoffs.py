#!/usr/bin/env python3
"""Extract Shanghai 2025 college admission cutoff scores from PDF to JSON."""

import json
import re
import sys

try:
    import pdfplumber
except ImportError:
    print("Installing pdfplumber...")
    import subprocess
    subprocess.check_call([sys.executable, "-m", "pip", "install", "pdfplumber"])
    import pdfplumber

PDF_PATH = r"e:\projects\gaokaoapply\上海市2025年普通高校招生本科普通批次平行志愿院校专业组投档分数线.pdf"
OUTPUT_PATH = r"e:\projects\gaokaoapply\src\data\gaokao\shanghai_2025_cutoffs.json"

def parse_value(v):
    """Convert string to int if possible, else return as-is."""
    if v is None:
        return None
    v = v.strip()
    # Strip non-numeric prefix characters (PDF artifacts like '上0')
    v = re.sub(r'^[^\d\-]+', '', v)
    try:
        return int(v)
    except ValueError:
        return v if v else None

def extract_rows(pdf_path):
    records = []
    with pdfplumber.open(pdf_path) as pdf:
        for page_num, page in enumerate(pdf.pages, 1):
            print(f"Processing page {page_num}/{len(pdf.pages)}...")
            tables = page.extract_tables()
            for table in tables:
                for row in table:
                    if not row or len(row) < 3:
                        continue
                    # Clean up cells
                    row = [c.replace('\n', '') if c else '' for c in row]
                    code = row[0].strip() if row[0] else ''
                    name = row[1].strip() if len(row) > 1 else ''
                    cutoff_raw = row[2].strip() if len(row) > 2 else ''
                    
                    # Skip header rows
                    if not code or not code[0].isdigit():
                        continue
                    if code == '院校专业' or name == '院校专业组':
                        continue
                    
                    # Parse cutoff
                    if '580' in cutoff_raw:
                        cutoff = "580分及以上"
                    else:
                        try:
                            cutoff = int(cutoff_raw)
                        except ValueError:
                            continue
                    
                    record = {"code": code, "name": name, "cutoff": cutoff}
                    
                    # If numeric cutoff, extract additional fields
                    if isinstance(cutoff, int) and len(row) >= 10:
                        fields = ['chinese_math', 'higher_score', 'english', 
                                  'subject1', 'subject2', 'subject3', 'bonus']
                        for i, field in enumerate(fields):
                            val = parse_value(row[3 + i] if 3 + i < len(row) else None)
                            if val is not None and val != '':
                                record[field] = val
                    
                    records.append(record)
                    
    return records

def main():
    print(f"Extracting from: {PDF_PATH}")
    records = extract_rows(PDF_PATH)
    print(f"Extracted {len(records)} records")
    
    output = {
        "title": "上海市2025年普通高校招生本科普通批次平行志愿院校专业组投档分数线",
        "year": 2025,
        "province": "上海",
        "note": "580分及以上考生投档信息另行告知；部分院校Q组、中外合作办学院校专业组投档结果另行公布",
        "data": records
    }
    
    with open(OUTPUT_PATH, 'w', encoding='utf-8') as f:
        json.dump(output, f, ensure_ascii=False, indent=2)
    
    print(f"Saved to: {OUTPUT_PATH}")

if __name__ == '__main__':
    main()
