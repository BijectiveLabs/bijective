// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Bijective Labs

//! The Parquet encoding of the two tables. The only module that names the format library, so
//! the library can be replaced without touching a caller.
//!
//! Written with fixed properties (zstd level 3, a fixed writer name, no wall time anywhere), so
//! the same rows always produce the same bytes. Read back through the physical columns, with the
//! schema and the platform's key-value metadata checked before any value is trusted.

use std::fs::File;
use std::path::Path;
use std::sync::Arc;

use parquet::basic::{Compression, ZstdLevel};
use parquet::column::reader::ColumnReader;
use parquet::column::writer::ColumnWriter;
use parquet::data_type::ByteArray;
use parquet::file::metadata::KeyValue;
use parquet::file::properties::WriterProperties;
use parquet::file::reader::{FileReader, SerializedFileReader};
use parquet::file::writer::SerializedFileWriter;
use parquet::schema::parser::parse_message_type;

use crate::columns::Column;
use crate::error::ArchiveError;

/// Raised when a table's layout changes. A reader refuses a file of another format.
pub const FORMAT: &str = "1";

const WRITER: &str = "bijective-archive";
const ZSTD_LEVEL: i32 = 3;

/// What a file declares about its contents, beside the schema.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableMeta {
    pub table: &'static str,
    pub venue: String,
    pub price_scale: u8,
    pub quantity_scale: u8,
}

impl TableMeta {
    fn pairs(&self) -> Vec<(&'static str, String)> {
        vec![
            ("bijective.format", FORMAT.to_string()),
            ("bijective.table", self.table.to_string()),
            ("bijective.venue", self.venue.clone()),
            ("bijective.price_scale", self.price_scale.to_string()),
            ("bijective.quantity_scale", self.quantity_scale.to_string()),
        ]
    }
}

pub fn schema_of(meta: &TableMeta) -> String {
    match meta.table {
        "book" => format!(
            "message book {{
              REQUIRED INT64 journal_seq (INTEGER(64,false));
              REQUIRED INT32 record_in_frame (INTEGER(32,false));
              REQUIRED INT64 received_at (TIMESTAMP(NANOS,true));
              OPTIONAL INT64 exchange_time (TIMESTAMP(NANOS,true));
              REQUIRED BYTE_ARRAY instrument (STRING);
              OPTIONAL INT64 sequence (INTEGER(64,false));
              REQUIRED BYTE_ARRAY update (STRING);
              OPTIONAL INT32 level (INTEGER(32,false));
              OPTIONAL BYTE_ARRAY side (STRING);
              OPTIONAL BYTE_ARRAY action (STRING);
              OPTIONAL INT64 price (DECIMAL(18,{}));
              OPTIONAL INT64 quantity (DECIMAL(18,{}));
            }}",
            meta.price_scale, meta.quantity_scale
        ),
        "facts" => format!(
            "message facts {{
              REQUIRED INT64 journal_seq (INTEGER(64,false));
              REQUIRED INT32 record_in_frame (INTEGER(32,false));
              REQUIRED INT64 received_at (TIMESTAMP(NANOS,true));
              OPTIONAL INT64 exchange_time (TIMESTAMP(NANOS,true));
              REQUIRED BYTE_ARRAY instrument (STRING);
              REQUIRED BYTE_ARRAY kind (STRING);
              OPTIONAL BYTE_ARRAY side (STRING);
              OPTIONAL BYTE_ARRAY trade_id (STRING);
              OPTIONAL INT64 price (DECIMAL(18,{}));
              OPTIONAL INT64 quantity (DECIMAL(18,{}));
              OPTIONAL INT64 value;
              OPTIONAL INT32 value_scale (INTEGER(8,false));
              OPTIONAL INT64 next_funding (TIMESTAMP(NANOS,true));
            }}",
            meta.price_scale, meta.quantity_scale
        ),
        _ => "message gaps {
              REQUIRED INT64 journal_seq (INTEGER(64,false));
              REQUIRED INT64 at (TIMESTAMP(NANOS,true));
              REQUIRED BYTE_ARRAY kind (STRING);
              REQUIRED BYTE_ARRAY detail (STRING);
            }"
        .to_string(),
    }
}

/// One row group per element of `groups`, in order. Postcondition: the bytes are a complete file.
pub fn encode(
    meta: &TableMeta,
    groups: &[Vec<Column>],
    path: &Path,
) -> Result<Vec<u8>, ArchiveError> {
    let schema =
        Arc::new(parse_message_type(&schema_of(meta)).map_err(|e| ArchiveError::parquet(path, e))?);
    let props = WriterProperties::builder()
        .set_compression(Compression::ZSTD(
            ZstdLevel::try_new(ZSTD_LEVEL).map_err(|e| ArchiveError::parquet(path, e))?,
        ))
        .set_created_by(WRITER.to_string())
        .set_key_value_metadata(Some(
            meta.pairs().into_iter().map(|(k, v)| KeyValue::new(k.to_string(), v)).collect(),
        ))
        .build();

    let mut out = Vec::new();
    let mut writer = SerializedFileWriter::new(&mut out, schema, Arc::new(props))
        .map_err(|e| ArchiveError::parquet(path, e))?;
    for columns in groups {
        let mut group = writer.next_row_group().map_err(|e| ArchiveError::parquet(path, e))?;
        for column in columns {
            let Some(mut target) =
                group.next_column().map_err(|e| ArchiveError::parquet(path, e))?
            else {
                return Err(ArchiveError::corrupt(path, "more columns than the schema declares"));
            };
            write_column(target.untyped(), column).map_err(|e| ArchiveError::parquet(path, e))?;
            target.close().map_err(|e| ArchiveError::parquet(path, e))?;
        }
        group.close().map_err(|e| ArchiveError::parquet(path, e))?;
    }
    writer.close().map_err(|e| ArchiveError::parquet(path, e))?;
    Ok(out)
}

fn write_column(target: &mut ColumnWriter<'_>, column: &Column) -> parquet::errors::Result<()> {
    let defs = column.defs();
    match (target, column) {
        (ColumnWriter::Int64ColumnWriter(w), Column::I64 { values, .. }) => {
            w.write_batch(values, defs, None)?;
        }
        (ColumnWriter::Int32ColumnWriter(w), Column::I32 { values, .. }) => {
            w.write_batch(values, defs, None)?;
        }
        (ColumnWriter::ByteArrayColumnWriter(w), Column::Text { values, .. }) => {
            let bytes: Vec<ByteArray> =
                values.iter().map(|v| ByteArray::from(v.as_bytes().to_vec())).collect();
            w.write_batch(&bytes, defs, None)?;
        }
        _ => {
            return Err(parquet::errors::ParquetError::General(
                "a column's values do not match its physical type".into(),
            ));
        }
    }
    Ok(())
}

/// Every row group's columns, in schema order, after the file's declarations are checked.
pub fn decode(meta: &TableMeta, path: &Path) -> Result<Vec<Vec<Column>>, ArchiveError> {
    let file = File::open(path).map_err(|e| ArchiveError::io(path, e))?;
    let reader = SerializedFileReader::new(file).map_err(|e| ArchiveError::parquet(path, e))?;
    check_declarations(meta, &reader, path)?;

    let mut groups = Vec::new();
    for g in 0..reader.num_row_groups() {
        let group = reader.get_row_group(g).map_err(|e| ArchiveError::parquet(path, e))?;
        let mut columns = Vec::new();
        for c in 0..group.num_columns() {
            let optional = group.metadata().column(c).column_descr().max_def_level() > 0;
            let column = group.get_column_reader(c).map_err(|e| ArchiveError::parquet(path, e))?;
            columns
                .push(read_column(column, optional).map_err(|e| ArchiveError::parquet(path, e))?);
        }
        groups.push(columns);
    }
    Ok(groups)
}

/// The price and quantity scales a file declares, for a reader that knows the venue and not its
/// scales. Only an expectation to `decode` against: nothing is read from the file on its word.
pub fn declared_scales(path: &Path) -> Result<(u8, u8), ArchiveError> {
    let file = File::open(path).map_err(|e| ArchiveError::io(path, e))?;
    let reader = SerializedFileReader::new(file).map_err(|e| ArchiveError::parquet(path, e))?;
    let declared = reader.metadata().file_metadata().key_value_metadata();
    let scale = |key: &str| {
        declared
            .and_then(|kv| kv.iter().find(|p| p.key == key))
            .and_then(|p| p.value.as_deref())
            .and_then(|v| v.parse::<u8>().ok())
            .ok_or_else(|| ArchiveError::corrupt(path, format!("{key} is not declared")))
    };
    Ok((scale("bijective.price_scale")?, scale("bijective.quantity_scale")?))
}

/// The schema text and the metadata both, so a file of another venue, scale or format is refused.
fn check_declarations(
    meta: &TableMeta,
    reader: &SerializedFileReader<File>,
    path: &Path,
) -> Result<(), ArchiveError> {
    let expected =
        parse_message_type(&schema_of(meta)).map_err(|e| ArchiveError::parquet(path, e))?;
    let found = reader.metadata().file_metadata().schema();
    if found != &expected {
        return Err(ArchiveError::corrupt(
            path,
            format!("schema is not the {} table's", meta.table),
        ));
    }
    let declared = reader.metadata().file_metadata().key_value_metadata();
    for (key, value) in meta.pairs() {
        let held =
            declared.and_then(|kv| kv.iter().find(|p| p.key == key)).and_then(|p| p.value.clone());
        if held.as_deref() != Some(value.as_str()) {
            return Err(ArchiveError::corrupt(
                path,
                format!("{key} is {held:?}, expected {value}"),
            ));
        }
    }
    Ok(())
}

/// Batch size only bounds each read call's buffer. It does not change what is read.
const READ_BATCH: usize = 8192;

fn read_column(column: ColumnReader, optional: bool) -> parquet::errors::Result<Column> {
    let mut defs: Vec<i16> = Vec::new();
    let out = match column {
        ColumnReader::Int64ColumnReader(mut r) => {
            let mut values = Vec::new();
            drain(|d, v| r.read_records(READ_BATCH, d, None, v), &mut defs, &mut values, optional)?;
            Column::I64 { values, defs: optional.then_some(defs) }
        }
        ColumnReader::Int32ColumnReader(mut r) => {
            let mut values = Vec::new();
            drain(|d, v| r.read_records(READ_BATCH, d, None, v), &mut defs, &mut values, optional)?;
            Column::I32 { values, defs: optional.then_some(defs) }
        }
        ColumnReader::ByteArrayColumnReader(mut r) => {
            let mut raw: Vec<ByteArray> = Vec::new();
            drain(|d, v| r.read_records(READ_BATCH, d, None, v), &mut defs, &mut raw, optional)?;
            let values = raw
                .iter()
                .map(|b| b.as_utf8().map(str::to_string))
                .collect::<parquet::errors::Result<Vec<String>>>()?;
            Column::Text { values, defs: optional.then_some(defs) }
        }
        _ => {
            return Err(parquet::errors::ParquetError::General(
                "a column of a type no table declares".into(),
            ));
        }
    };
    Ok(out)
}

/// Reads into fresh buffers each call and appends, so the result does not depend on whether the
/// library appends to or overwrites a buffer it is handed.
fn drain<T: Clone>(
    mut call: impl FnMut(
        Option<&mut Vec<i16>>,
        &mut Vec<T>,
    ) -> parquet::errors::Result<(usize, usize, usize)>,
    defs: &mut Vec<i16>,
    values: &mut Vec<T>,
    optional: bool,
) -> parquet::errors::Result<()> {
    loop {
        let mut d: Vec<i16> = Vec::new();
        let mut v: Vec<T> = Vec::new();
        let (records, _, _) = call(optional.then_some(&mut d), &mut v)?;
        if records == 0 {
            return Ok(());
        }
        defs.extend(d);
        values.extend(v);
    }
}
