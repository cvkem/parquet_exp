use std::{
    fs,
    io::{Write, BufWriter},
    mem,
    sync::{Arc, mpsc::{self, SyncSender}},
    thread,
};
use parquet::{
    errors::Result,
    record::{Field, Row},
    schema::types::Type
};
use crate::rowwriter;

pub struct RowWriteBuffer {
    max_row_group: usize,
    buffer: Vec<Row>,
    write_sink: SyncSender<Vec<Row>>,
    writer_handle: thread::JoinHandle<()>
}


impl RowWriteBuffer {
    
    pub fn new(path: &str, schema: Arc<Type>, group_size: usize) -> Result<RowWriteBuffer> {
        let (write_sink, rec_buffer) = mpsc::sync_channel(2);

        let schema_clone = schema.clone();
        let path_clone = path.to_owned();

        let writer_handle = thread::spawn(move || {
            let writer = create_writer(&path_clone);
            match rowwriter::RowWriter::channel_writer(rec_buffer, writer, schema_clone) {
                Ok(()) => println!("File {path_clone:?} written"),
                Err(err) => println!("Writing file failed with errors {:?}", err)
            }
        });

        let row_writer = RowWriteBuffer {
            max_row_group: group_size,
            buffer: Vec::with_capacity(group_size),
            write_sink,
            writer_handle
        };
    
        Ok(row_writer)
    }

    pub fn remaining_space(&self) -> usize {
        self.max_row_group - self.buffer.len()
    }


    pub fn flush(&mut self) -> Result<()> {
        let rows_to_write = mem::take(&mut self.buffer);

        self.write_sink.send(rows_to_write).unwrap(); // can not use ?  should use match to propagate error.

        Ok(())
    }

    pub fn append_row(&mut self, row: Row) {
        self.buffer.push(row);

        if self.buffer.len() == self.max_row_group {
            self.flush().expect("Failed to flush buffer");
            self.buffer.clear();
        }
    }

    // pub fn write_duration(&self) -> Duration {
    //     self.duration.clone()
    // }

    // Close does consume the writer. 
    // Possibly does this work well when combined with a drop trait?
    pub fn close(mut self)  {
        if self.buffer.len() > 0 {
            if let Err(err) = self.flush() {
                panic!("auto-Flush on close failed with {err}");
            }
        }

        println!("Closing the sending end of the channel.");
        // closing channel will close the writer
        drop(self.write_sink);

        // wait for writer to be ready
        self.writer_handle.join().unwrap();

    }
}


// // failed to implement drop as it requires and owned value
// impl<W> Drop for RowWriter<W> where 
//     W: Write {
//     fn drop(&mut self) {
//         self.close();
//     }
// }


/// Create a writer based on a string that implements the std::io::Write interface.
/// If string is prefixed by 'mem:' this will be an in memory buffer, if is is prefixed by 's3:' it will be a s3-object. Otherswise it will be a path on the local file system. 
fn create_writer(path: &str) -> Box<dyn Write> {
    let writer: Box<dyn Write> = match path.split(':').next().unwrap() {
        prefix if prefix.len() == path.len() => {
                let file = fs::OpenOptions::new()
//                    .read(true)
                    .write(true)
                    .create(true)
                    .truncate(true)
                    .open(path)
                    .unwrap();
                Box::new(BufWriter::new(file))
            },
        "mem" => Box::new(Vec::new()),
//        "s3" => println!("{s}: S3"),
        prefix => panic!("Unknown prefix '{prefix}' on file {path}")
    };
    writer
}


/// Creates a frow from a series of tuples. This function is based on parquet::record::api::make_row, which is a private function.
/// A transmute is used to be able to create the rows here. This is a safe step as both parquet::record::Row and RowImitation have the same 
/// definition, both are compiled with the same compiler, and a struct with only 1 field allows for only a single logical layout.
pub fn create_row(fields: Vec<(String, Field)>) -> Row {
    
    pub struct RowImitation {
        fields: Vec<(String, Field)>,
    }
    let row_contents = RowImitation { fields };
    unsafe {mem::transmute(row_contents)}
}

#[cfg(test)]
pub mod tests {

    use std::{
        fs::File,
        path::Path,
        sync::Arc};
    use parquet::{
        basic::Compression,
        data_type::{Int32Type, Int64Type, ByteArrayType, ByteArray},
        file::{
            properties::WriterProperties,
            writer::{
                SerializedFileWriter,
                SerializedRowGroupWriter},
            reader::{
                SerializedFileReader,
                FileReader}
        },
        record::{Row, RowAccessor, Field},
        schema::{parser::parse_message_type,
            types::Type}
    };
    use crate::rowwritebuffer;
    use crate::rowiterext;


    // this is not the right test as I switch to example code
    #[test]
    fn test_write_parquet() {
        const MESSAGE_TYPE: &str = "
        message schema {
            REQUIRED INT64 id;
            REQUIRED BINARY account (UTF8);
        ";
        let input_tuples = vec![(1_i64, "Hello".to_owned()), (2_i64, "World".to_owned()), (3_i64, "This is a test!".to_owned())];

        let tuple_to_row = |(id, account)|  rowwritebuffer::create_row(vec![("id".to_owned(), Field::Long(id)), ("account".to_owned(), Field::Str(account))]); 
        let input_rows: Vec<Row> = input_tuples
            .clone()
            .into_iter()
            .map(tuple_to_row)
            .collect(); 

        let path = "/tmp/test_write_parquet.parquet";
//        let path = "test_write_parquet.parquet";
        let schema = Arc::new(parse_message_type(MESSAGE_TYPE).unwrap());


        let mut row_writer = rowwritebuffer::RowWriteBuffer::new(path, schema, 10_000).unwrap();

        for row in input_rows.into_iter() {
            row_writer.append_row(row);
        }


        row_writer.close();

        println!("Now open the file {path} and read it again");
        let result = rowiterext::read_parquet_rowiter(path, Some(10), MESSAGE_TYPE);

        println!("Result of read: {}", result[0]);
        let output_tuples: Vec<(i64, String)> = result
            .iter()
            .map(|row| (row.get_long(0).unwrap(), row.get_string(1).unwrap().to_owned()))
            .collect();
        assert_eq!(input_tuples, output_tuples)

    }

}