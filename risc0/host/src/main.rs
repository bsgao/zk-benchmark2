// These constants represent the RISC-V ELF and the image ID generated by risc0-build.
// The ELF is used for proving and the ID is used for verification.
use csv::ReaderBuilder;
use host::run_model;
use methods::{ML_GUEST_ELF, ML_GUEST_ID};
use risc0_zkvm::{default_prover, ExecutorEnv};
use serde::{Deserialize, Serialize};
use serde_json::from_str;
use std::env;
use std::fmt;
use std::fs::File;
use std::io;
use std::io::Read;
use std::time::Instant;

use ml_core::{
    LinearRegressionModel, LinearRegressionParams, ModelInput, RidgeRegressionModel,
    RidgeRegressionParams, Scaler, ScalerParams,
};

#[derive(Debug, Deserialize)]
struct TestData {
    CustomerID: f32,
    frequency: f32,
    monetary: f32,
    recency: f32,
    Price: f32,
    DiscountApplied: f32,
    spend_90_flag: f32,
    Actual_spend_90_days: f32,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum MyError {
    FileNotFound(String),
    ParseError(String),
    IoError(String),
    InvalidInput(String),
}

impl fmt::Display for MyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MyError::FileNotFound(msg) => write!(f, "File not found: {}", msg),
            MyError::ParseError(msg) => write!(f, "Parse error: {}", msg),
            MyError::IoError(msg) => write!(f, "IO error: {}", msg),
            MyError::InvalidInput(msg) => write!(f, "Invlaid Input: {}", msg),
        }
    }
}

impl std::error::Error for MyError {}

impl From<io::Error> for MyError {
    fn from(error: io::Error) -> Self {
        MyError::IoError(error.to_string())
    }
}

impl From<serde_json::Error> for MyError {
    fn from(error: serde_json::Error) -> Self {
        MyError::ParseError(error.to_string())
    }
}

pub fn read_test_dataset(lines: usize) -> Result<(Vec<Vec<f32>>, Vec<f32>), MyError> {
    match env::current_dir() {
        Ok(path) => println!("Current working directory: {}", path.display()),
        Err(e) => eprintln!("Error getting current directory: {}", e),
    }
    let mut file =
        File::open("./host/model/Test_Dataset.csv").expect("Test_Dataset.csv not found.");
    let mut contents = String::new();
    file.read_to_string(&mut contents)
        .expect("Failed to read Test_Dataset.csv");

    // Parse the CSV into a vector of TestData
    let mut rdr = ReaderBuilder::new()
        .has_headers(true)
        .from_reader(contents.as_bytes());

    let mut test_features = Vec::new();
    let mut actual_amounts = Vec::new();
    let mut cur: usize = 0;
    for result in rdr.deserialize() {
        if cur == lines {
            break;
        }
        let record: TestData = result.expect("Failed to deserialize record.");
        test_features.push(vec![
            record.CustomerID,
            record.frequency,
            record.monetary,
            record.recency,
            record.Price,
            record.DiscountApplied,
            record.spend_90_flag
        ]);
        actual_amounts.push(record.Actual_spend_90_days);
        cur += 1;
    }

    Ok((test_features, actual_amounts))
}

pub fn read_models(
    scaler_path: &str,
    linear_model_path: &str,
    ridge_model_path: &str,
    poly_ridge_model_path: &str,
) -> Result<(Scaler, LinearRegressionModel, RidgeRegressionModel), MyError> {
    match env::current_dir() {
        Ok(path) => println!(
            "Current working directory in read_model: {}",
            path.display()
        ),
        Err(e) => eprintln!("Error getting current directory: {}", e),
    }
    // Read and deserialize Scaler
    let mut scaler_file = File::open(scaler_path)?;
    let mut scaler_contents = String::new();
    scaler_file.read_to_string(&mut scaler_contents)?;
    let scalar_params: ScalerParams = from_str(&scaler_contents)?;
    let scaler = Scaler::new(scalar_params);

    // Read and deserialize LinearRegressionModel
    let mut linear_file = File::open(linear_model_path)?;
    let mut linear_contents = String::new();
    linear_file.read_to_string(&mut linear_contents)?;
    let linear_params: LinearRegressionParams = from_str(&linear_contents)?;
    let linear_model = LinearRegressionModel::new(linear_params);

    // Read and deserialize RidgeRegressionModel
    let mut ridge_file = File::open(ridge_model_path)?;
    let mut ridge_contents = String::new();
    ridge_file.read_to_string(&mut ridge_contents)?;
    let ridge_params: RidgeRegressionParams = from_str(&ridge_contents)?;
    let ridge_model = RidgeRegressionModel::new(ridge_params);

    Ok((scaler, linear_model, ridge_model))
}

fn mean_squared_error(actual: &[f32], predicted: &[f32]) -> f32 {
    assert_eq!(
        actual.len(),
        predicted.len(),
        "The length of actual and predicted slices must be the same."
    );

    let mse = actual.iter()
        .zip(predicted.iter())
        .map(|(a, p)| (a - p).powi(2))
        .sum::<f32>() / (actual.len() as f32);

    mse
}

fn r2_score(actual: &[f32], predicted: &[f32]) -> f32 {
    assert_eq!(
        actual.len(),
        predicted.len(),
        "The length of actual and predicted slices must be the same."
    );

    let mean_actual = actual.iter().copied().sum::<f32>() / (actual.len() as f32);

    let ss_tot = actual.iter()
        .map(|&a| (a - mean_actual).powi(2))
        .sum::<f32>();

    let ss_res = actual.iter()
        .zip(predicted.iter())
        .map(|(a, p)| (a - p).powi(2))
        .sum::<f32>();

    if ss_tot == 0.0 {
        // Avoid division by zero; return R² = 1 if predictions are perfect, else 0
        if ss_res == 0.0 {
            1.0
        } else {
            0.0
        }
    } else {
        1.0 - (ss_res / ss_tot)
    }
}

fn main() {
    let scaler_path = "./host/model/scaler_params.json";
    let linear_model_path = "./host/model/linear_regression_params.json";
    let ridge_model_path = "./host/model/ridge_regression_params.json";
    let poly_ridge_model_path = "./host/model/polynomial_ridge_regression_params.json";

    // Read the test dataset
    let Ok((x, actual_amounts)) = read_test_dataset(100) else {
        todo!()
    };

    // Read the models
    let Ok((scaler, linear_model, ridge_model)) = read_models(
        scaler_path,
        linear_model_path,
        ridge_model_path,
        poly_ridge_model_path,
    ) else {
        eprintln!(
            "{:?}",
            read_models(
                scaler_path,
                linear_model_path,
                ridge_model_path,
                poly_ridge_model_path
            )
        );

        return; // Exit or take alternative action
    };

    let model_input = ModelInput {
        scaler,
        ridge_model,
        x,
    };

    // Initialize tracing. In order to view logs, run `RUST_LOG=info cargo run`
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::filter::EnvFilter::from_default_env())
        .init();


    // Obtain the default prover.
    let prover = default_prover();

    // Proof information by proving the specified ELF binary.
    // This struct contains the receipt along with statistics about execution of the guest
    println!("Start generating proof");
    let mut start = Instant::now();
    let receipt = run_model(model_input).unwrap();
    let proof_duration = start.elapsed();
    start = Instant::now(); 
    let verify = receipt.verify();
    let verify_duration = start.elapsed();
    let output = receipt.get_commit().unwrap();

    let mse = mean_squared_error(&output, &actual_amounts);
    println!("mean squared error (mse): {:.4}", mse);

    // calculate r²
    let r2 = r2_score(&output, &actual_amounts);
    println!("coefficient of determination (r²): {:.4}", r2);
    println!("{:?}", output);
    println!("{:?}", actual_amounts);
    println!("Proof time taken: {:?}", proof_duration);
    println!("Verify time taken: {:?}", verify_duration);

    // The receipt was verified at the end of proving, but the below code is an
    // example of how someone else could verify this receipt.
}
