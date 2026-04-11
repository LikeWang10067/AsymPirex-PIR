// src/database.rs

pub struct Database {
    pub total_records: usize, // Total number of records N
    pub partition_count: usize, // Number of partitions n
    pub records_per_partition: usize, // Number of records per partition m
    pub record_size: usize, // Record size B (Bytes)
    pub data: Vec<Vec<Vec<u8>>>, // Data matrix in memory [n][m][B]
}

impl Database {
    /// Phase 1: System Initialization
    pub fn new(n: usize, m: usize, record_size: usize) -> Self {
        let total_records = n * m;
        let mut data = Vec::with_capacity(n);

        for _ in 0..n {
            let mut partition = Vec::with_capacity(m);
            for _ in 0..m {
                // Fill initial records with zeros
                let record = vec![0u8; record_size];
                partition.push(record);
            }
            data.push(partition);
        }

        Database {
            total_records,
            partition_count: n,
            records_per_partition: m,
            record_size,
            data,
        }
    }

    /// Get record by global index x, corresponding to k = floor(x / m)
    pub fn get_record(&self, x: usize) -> Option<&Vec<u8>> {
        if x >= self.total_records {
            return None; 
        }
        let k = x / self.records_per_partition;
        let offset = x % self.records_per_partition;
        Some(&self.data[k][offset])
    }

    pub fn set_record(&mut self, x: usize, record: Vec<u8>) {
        assert!(x < self.total_records, "record index out of range");
        assert_eq!(
            record.len(),
            self.record_size,
            "record length must match the configured record size"
        );

        let k = x / self.records_per_partition;
        let offset = x % self.records_per_partition;
        self.data[k][offset] = record;
    }
}