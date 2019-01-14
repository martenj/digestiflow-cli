//! Implementation of flow cell folder analysis and import.

use restson::RestClient;
use std::collections::HashMap;
use std::env;
use std::fs::File;
use std::io::prelude::*;
use std::path::Path;
use std::result;
use sxd_document::parser;

use super::errors::*;
use settings::Settings;

mod api;
mod bcl_meta;
use self::bcl_meta::*;
mod bcl_data;
use self::bcl_data::*;

/// Build a flow cell from the meta information in `run_info` and `run_params`.
///
/// When provided, the previous/current status of sequencing can be given in `status_sequencing`.
fn build_flow_cell(
    run_info: &RunInfo,
    run_params: &RunParameters,
    path: &Path,
    status_sequencing: Option<String>,
    settings: &Settings,
) -> api::FlowCell {
    api::FlowCell {
        sodar_uuid: None,
        run_date: run_info.date.clone(),
        run_number: run_info.run_number,
        slot: run_params.flowcell_slot.clone(),
        vendor_id: run_info.flowcell.clone(),
        label: Some(run_params.experiment_name.clone()),
        num_lanes: run_info.lane_count,
        rta_version: if run_params.rta_version.starts_with(&"2") {
            2
        } else {
            1
        },
        planned_reads: Some(string_description(&run_params.planned_reads)),
        current_reads: Some(string_description(&run_info.reads)),
        manual_label: None,
        description: None,
        sequencing_machine: run_info.instrument.clone(),
        operator: Some(settings.ingest.operator.clone()),
        status_sequencing: get_status_sequencing(
            run_info,
            run_params,
            path,
            &status_sequencing.unwrap_or("initial".to_string()),
        ),
        status_conversion: "initial".to_string(),
        status_delivery: "initial".to_string(),
        delivery_type: "seq".to_string(),
    }
}

/// Register a new flow cell with the REST API given the information in `run_info` and `run_params`.
fn register_flowcell(
    logger: &slog::Logger,
    client: &mut RestClient,
    run_info: &RunInfo,
    run_params: &RunParameters,
    path: &Path,
    settings: &Settings,
) -> Result<api::FlowCell> {
    info!(logger, "Registering flow cell...");

    let flowcell = build_flow_cell(run_info, run_params, path, None, settings);
    debug!(logger, "Registering flowcell with API as {:?}", &flowcell);

    let args = api::ProjectArgs {
        project_uuid: settings.ingest.project_uuid.clone(),
    };
    let flowcell = client
        .post_capture(&args, &flowcell)
        .chain_err(|| "Problem registering data")?;
    debug!(logger, "Registered flowcell: {:?}", &flowcell);

    info!(logger, "Done registering flow cell.");

    Ok(flowcell)
}

/// Register an existing flow cell with the REST API given the information in `run_info` and `run_params`.
fn update_flowcell(
    logger: &slog::Logger,
    client: &mut RestClient,
    flowcell: &api::FlowCell,
    run_info: &RunInfo,
    run_params: &RunParameters,
    path: &Path,
    settings: &Settings,
) -> Result<api::FlowCell> {
    info!(logger, "Updating flow cell...");

    let rebuilt = build_flow_cell(
        run_info,
        run_params,
        path,
        Some(flowcell.status_sequencing.clone()),
        settings,
    );

    let flowcell = api::FlowCell {
        planned_reads: rebuilt.planned_reads.clone(),
        current_reads: rebuilt.current_reads.clone(),
        status_sequencing: rebuilt.status_sequencing.clone(),
        ..flowcell.clone()
    };
    info!(logger, "Updating flow cell via API");
    debug!(logger, "  {:?} => {:?}", &flowcell, &rebuilt);

    let args = api::ProjectFlowcellArgs {
        project_uuid: settings.ingest.project_uuid.clone(),
        flowcell_uuid: flowcell.sodar_uuid.clone().unwrap(),
    };
    client
        .put_capture(&args, &flowcell)
        .chain_err(|| "Problem updating")
}

/// Kick of analyzing the adatpers and then update through API if configured to do so in `settings`.
fn analyze_adapters(
    logger: &slog::Logger,
    flowcell: &api::FlowCell,
    client: &mut RestClient,
    run_info: &RunInfo,
    path: &Path,
    folder_layout: FolderLayout,
    settings: &Settings,
) -> Result<()> {
    info!(logger, "Analyzing adapters...");

    let mut index_no = 0i32;
    let mut cycle = 1i32; // always throw away first cycle
    for ref desc in &run_info.reads {
        if desc.is_index {
            index_no += 1;
            let index_counts = sample_adapters(
                logger,
                path,
                &desc,
                folder_layout,
                settings,
                index_no,
                cycle,
            )?;

            // Push results to API
            if settings.ingest.post_adapters {
                info!(
                    logger,
                    "Updating adapter information via API {:?}", &flowcell
                );
                for (i, index_info) in index_counts.iter().enumerate() {
                    let lane_no = i + 1;
                    let api_hist = api::LaneIndexHistogram {
                        sodar_uuid: None,
                        flowcell: flowcell.sodar_uuid.clone().unwrap(),
                        lane: lane_no as i32,
                        index_read_no: index_no,
                        sample_size: index_info.sample_size,
                        histogram: index_info.hist.clone(),
                    };
                    client
                        .post(
                            &api::ProjectFlowcellArgs {
                                project_uuid: settings.ingest.project_uuid.clone(),
                                flowcell_uuid: flowcell.sodar_uuid.clone().unwrap(),
                            },
                            &api_hist,
                        ).chain_err(|| "Could not update adapter on server")?
                }
            }
        }
        cycle += desc.num_cycles;
    }

    info!(logger, "Done analyzing adapters.");
    Ok(())
}

/// Process the sequencer output folder at `path` with the given `settings`.
fn process_folder(logger: &slog::Logger, path: &Path, settings: &Settings) -> Result<()> {
    info!(logger, "Starting to process folder {:?}...", path);

    // Ensure that `RunInfo.xml` exists and try to guess folder layout.
    if !path.join("RunInfo.xml").exists() {
        error!(
            logger,
            "Path {:?}/RunInfo.xml does not exist! Skipping directory.", path
        );
        bail!("RunInfo.xml missing");
    }
    let folder_layout = match guess_folder_layout(path) {
        Ok(layout) => layout,
        Err(_e) => {
            error!(
                logger,
                "Could not guess folder layout from {:?}. Skipping.", path
            );
            bail!("Could not guess folder layout");
        }
    };

    // Parse the run info and run parameters XML files
    info!(logger, "Parsing XML files...");
    let info_pkg = {
        let mut xmlf =
            File::open(path.join("RunInfo.xml")).chain_err(|| "Problem reading RunInfo.xml")?;
        let mut contents = String::new();
        xmlf.read_to_string(&mut contents)
            .chain_err(|| "Problem reading XML from RunInfo.xml")?;
        parser::parse(&contents).chain_err(|| "Problem parsing XML from RunInfo.xml")?
    };
    let info_doc = info_pkg.as_document();

    let param_pkg = {
        let filename = match folder_layout {
            FolderLayout::MiSeq => "runParameters.xml",
            FolderLayout::MiniSeq => "RunParameters.xml",
            FolderLayout::HiSeqX => bail!("Cannot handle HiSeq X yet!"),
        };
        let mut xmlf = File::open(path.join(filename))
            .chain_err(|| format!("Problem reading {}", &filename))?;
        let mut contents = String::new();
        xmlf.read_to_string(&mut contents)
            .chain_err(|| format!("Problem reading XML from {}", &filename))?;
        parser::parse(&contents).chain_err(|| format!("Problem parsing XML from {}", &filename))?
    };
    let param_doc = param_pkg.as_document();

    // Process the XML files.
    let (run_info, run_params) = process_xml(logger, folder_layout, &info_doc, &param_doc)?;

    // Try to get the flow cell information from API.
    debug!(logger, "Connecting to \"{}\"", &settings.web.url);
    if settings.log_token {
        debug!(
            logger,
            "  (using header 'Authorization: Token {}')", &settings.web.token
        );
    }
    let mut client = RestClient::new(&settings.web.url).unwrap();
    client
        .set_header("Authorization", &format!("Token {}", &settings.web.token))
        .chain_err(|| "Problem configuring REST client")?;
    let result: result::Result<api::FlowCell, restson::Error> =
        client.get(&api::ResolveFlowCellArgs {
            project_uuid: settings.ingest.project_uuid.clone(),
            instrument: run_info.instrument.clone(),
            run_number: run_info.run_number,
            flowcell: run_info.flowcell.clone(),
        });

    let flowcell: api::FlowCell = if settings.ingest.register || settings.ingest.update {
        // Update or create if necessary.
        match result {
            Ok(flowcell) => {
                debug!(logger, "Flow cell found with value {:?}", &flowcell);
                if settings.ingest.update {
                    update_flowcell(
                        logger,
                        &mut client,
                        &flowcell,
                        &run_info,
                        &run_params,
                        &path,
                        &settings,
                    )?
                } else {
                    flowcell
                }
            }
            Err(restson::Error::HttpError(404, _msg)) => {
                debug!(logger, "Flow cell was not found!");
                if settings.ingest.register {
                    let flowcell = register_flowcell(
                        logger,
                        &mut client,
                        &run_info,
                        &run_params,
                        &path,
                        &settings,
                    )?;
                    debug!(logger, "Flow cell registered as {:?}", &flowcell);
                    flowcell
                } else {
                    info!(
                        logger,
                        "Flow cell was not found but you asked me not to \
                         register. Stopping here for this folder without \
                         error."
                    );
                    return Ok(());
                }
            }
            _x => bail!("Problem resolving flowcell {:?}", &_x),
        }
    } else {
        // TODO: improve error handling
        result.expect("Flowcell not found but we are not supposed to register")
    };

    // Check if we should skip this directory.
    if flowcell.status_sequencing != "initial" && flowcell.status_sequencing != "in_progress" {
        if settings.ingest.skip_if_status_final {
            info!(
                logger,
                "Flowcell has a final sequencing status ({:?}), skippping",
                &flowcell.status_sequencing
            );
            return Ok(());
        }
    }

    if settings.ingest.analyze_adapters {
        analyze_adapters(
            logger,
            &flowcell,
            &mut client,
            &run_info,
            &path,
            folder_layout,
            &settings,
        )?;
    } else {
        info!(logger, "You asked me to not analyze adapters.");
    }

    info!(logger, "Done processing folder {:?}.", path);
    Ok(())
}

/// Main entry point for the `ingest` command.
///
/// The function will skip folders for which errors occured but only return `Ok(())` if processing
/// all folders worked.
pub fn run(logger: &slog::Logger, settings: &Settings) -> Result<()> {
    info!(logger, "Running: digestiflow-cli-client ingest");
    info!(logger, "Options: {:?}", settings);
    env::set_var("RAYON_NUM_THREADS", format!("{}", settings.threads));

    // Bail out in case of missing project UUID.
    if settings.ingest.project_uuid.is_empty() {
        bail!("You have to specify the project UUID");
    }

    // Setting number of threads to use in Rayon.
    debug!(logger, "Using {} threads", settings.threads);
    env::set_var("RAYON_NUM_THREADS", format!("{}", settings.threads));

    let any_failed: bool = settings.ingest.path./*par_*/iter().map(|ref path| {
        let path = Path::new(path);
        match process_folder(logger, &path, settings) {
            Err(e) => {
                error!(logger, "Folder processing failed: {:?}", &e);
                warn!(
                    logger,
                    "Processing folder {:?} failed. Will go on with other paths but the program \
                     call will not have return code 0!",
                    &path
                );
                true // == any failed
            }
            _ => false,  // == any failed
        }
    }).any(|failed| failed);

    if any_failed {
        bail!("Processing of at least one folder failed!")
    } else {
        Ok(())
    }
}