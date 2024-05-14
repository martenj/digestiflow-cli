//! Code for accessing data in the raw output directories.

use chrono::{NaiveDate, NaiveDateTime};
use std::path::Path;
use sxd_document::dom::Document;
use sxd_xpath::nodeset::Node;
use sxd_xpath::{evaluate_xpath, Value};

use super::super::errors::*;

#[derive(PartialEq, Eq, Debug, Copy, Clone)]
pub enum FolderLayout {
    /// MiSeq (Windows XP), HiSeq 2000, etc. `runParameters.xml`
    MiSeqDep,
    /// MiniSeq, NextSeq etc. `RunParameters.xml`
    MiniSeq,
    /// HiSeq X
    HiSeqX,
    /// NovaSeq
    NovaSeq,
    /// MiSeq (Windows 10)
    MiSeq,
    /// NovaSeq X plus
    NovaSeqXplus,
    /// NextSeq 1000/2000
    NextSeq2000,
}

pub fn guess_folder_layout(path: &Path) -> Result<FolderLayout> {
    let miniseq_marker = vec![
        path.join("Data")
            .join("Intensities")
            .join("BaseCalls")
            .join("L001"),
        path.join("RunParameters.xml"),
    ];
    let miseqdep_marker = vec![
        path.join("Data")
            .join("Intensities")
            .join("BaseCalls")
            .join("L001")
            .join("C1.1"),
        path.join("runParameters.xml"),
    ];
    let miseq_marker = vec![
        path.join("Data")
            .join("Intensities")
            .join("BaseCalls")
            .join("L001")
            .join("C1.1"),
        path.join("RunParameters.xml"),
    ];
    let hiseqx_marker = vec![
        path.join("Data").join("Intensities").join("s.locs"),
        path.join("RunParameters.xml"),
    ];
    let novaseq_marker_any = vec![
        path.join("Data")
            .join("Intensities")
            .join("BaseCalls")
            .join("L001")
            .join("C1.1")
            .join("L001_1.cbcl"),
        path.join("Data")
            .join("Intensities")
            .join("BaseCalls")
            .join("L001")
            .join("C1.1")
            .join("L001_2.cbcl"),
    ];
    let novaseq_marker_all = vec![path.join("RunParameters.xml")];
//    let novaseqxplus_marker = vec![path.join("Manifest.tsv")];
    let linux_os_marker = vec![path.join("InstrumentAnalyticsLogs")];
    let novaseqxplus_marker = vec![path.join("RTAExited.txt")];

    if novaseq_marker_all.iter().all(|ref m| m.exists())
        && novaseq_marker_any.iter().any(|ref m| m.exists())
    {
       if linux_os_marker.iter().any(|ref m| m.exists()) {
           if novaseqxplus_marker.iter().any(|ref m| m.exists()) {
               Ok(FolderLayout::NovaSeqXplus)
           } else {
               Ok(FolderLayout::NextSeq2000)
           }
        } else {
            Ok(FolderLayout::NovaSeq)
        }
     } else if miseqdep_marker.iter().all(|ref m| m.exists()) {
        Ok(FolderLayout::MiSeqDep)
    } else if miseq_marker.iter().all(|ref m| m.exists()) {
        Ok(FolderLayout::MiSeq)
    } else if miniseq_marker.iter().all(|ref m| m.exists()) {
        Ok(FolderLayout::MiniSeq)
    } else if hiseqx_marker.iter().all(|ref m| m.exists()) {
        Ok(FolderLayout::HiSeqX)
    } else {
        bail!("Could not guess folder layout from {:?}", path)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct ReadDescription {
    pub number: i32,
    pub num_cycles: i32,
    pub is_index: bool,
}

pub fn string_description(read_descs: &Vec<ReadDescription>) -> String {
    read_descs
        .iter()
        .map(|x| format!("{}{}", x.num_cycles, if x.is_index { "B" } else { "T" }))
        .collect::<Vec<String>>()
        .join("")
}

#[derive(Debug)]
pub struct RunInfo {
    /// The long, full run ID.
    pub run_id: String,
    pub run_number: i32,
    pub flowcell: String,
    pub instrument: String,
    pub date: String,
    pub lane_count: i32,
    pub reads: Vec<ReadDescription>,
}

pub fn process_xml_run_info(info_doc: &Document) -> Result<RunInfo> {
    let reads = if let Value::Nodeset(nodeset) =
        evaluate_xpath(&info_doc, "//RunInfoRead|//Read")
            .chain_err(|| "Problem finding Read or RunInfoRead tags")?
    {
        let mut reads = Vec::new();
        for node in nodeset.document_order() {
            if let Node::Element(elem) = node {
                let num_cycles = elem
                    .attribute("NumCycles")
                    .expect("Problem accessing NumCycles attribute")
                    .value()
                    .to_string()
                    .parse::<i32>()
                    .unwrap();
                if num_cycles > 0 {
                    reads.push(ReadDescription {
                        number: elem
                            .attribute("Number")
                            .expect("Problem accessing Number attribute")
                            .value()
                            .to_string()
                            .parse::<i32>()
                            .unwrap(),
                        num_cycles: num_cycles,
                        is_index: elem
                            .attribute("IsIndexedRead")
                            .expect("Problem accessing IsIndexedRead attribute")
                            .value()
                            == "Y",
                    })
                }
            } else {
                bail!("Read was not a tag!")
            }
        }
        reads
    } else {
        bail!("Problem getting Read or RunInfoRead elements")
    };

    let xml_date = evaluate_xpath(&info_doc, "//Date/text()")
        .chain_err(|| "Problem reading //Date/text()")?
        .into_string();
    let date_string = if let Ok(good) = NaiveDate::parse_from_str(&xml_date, "%y%m%d") {
        good.format("%F").to_string()
    } else {
        if let Ok(good) = NaiveDateTime::parse_from_str(&xml_date, "%-m/%-d/%Y %-I:%M:%S %p") {
            good.format("%F").to_string()
        } else if let Ok(good) = NaiveDateTime::parse_from_str(&xml_date, "%Y-%m-%dT%H:%M:%SZ") {
            good.format("%F").to_string()
        } else {
            bail!("Could not parse date from string {}", &xml_date);
        }
    };

    Ok(RunInfo {
        run_id: evaluate_xpath(&info_doc, "//Run/@Id")
            .chain_err(|| "Problem reading //Run/@Id")?
            .into_string(),
        run_number: evaluate_xpath(&info_doc, "//Run/@Number")
            .chain_err(|| "Problem reading //Run/@Number")?
            .into_number() as i32,
        flowcell: evaluate_xpath(&info_doc, "//Flowcell/text()")
            .chain_err(|| "Problem reading //Flowcell/text()")?
            .into_string(),
        instrument: evaluate_xpath(&info_doc, "//Instrument/text()")
            .chain_err(|| "Problem reading //Instrument/text()")?
            .into_string(),
        date: date_string,
        lane_count: evaluate_xpath(&info_doc, "//FlowcellLayout/@LaneCount")
            .chain_err(|| "Problem reading //FlowcellLayout/@LaneCount")?
            .into_number() as i32,
        reads: reads,
    })
}

#[derive(Debug)]
pub struct RunParameters {
    pub planned_reads: Vec<ReadDescription>,
    pub rta_version: String,
    pub run_number: i32,
    pub flowcell_slot: String,
    pub experiment_name: String,
}

pub fn process_xml_param_doc_miseq(info_doc: &Document) -> Result<RunParameters> {
    let reads = if let Value::Nodeset(nodeset) =
        evaluate_xpath(&info_doc, "//RunInfoRead|//Read")
            .chain_err(|| "Problem finding Read or RunInfoRead tags")?
    {
        let mut reads = Vec::new();
        for node in nodeset.document_order() {
            if let Node::Element(elem) = node {
                let num_cycles = elem
                    .attribute("NumCycles")
                    .expect("Problem accessing NumCycles attribute")
                    .value()
                    .to_string()
                    .parse::<i32>()
                    .unwrap();
                if num_cycles > 0 {
                    reads.push(ReadDescription {
                        number: elem
                            .attribute("Number")
                            .expect("Problem accessing Number attribute")
                            .value()
                            .to_string()
                            .parse::<i32>()
                            .unwrap(),
                        num_cycles: num_cycles,
                        is_index: elem
                            .attribute("IsIndexedRead")
                            .expect("Problem accessing IsIndexedRead attribute")
                            .value()
                            == "Y",
                    })
                }
            } else {
                bail!("Read or RunInfoRead was not a tag!")
            }
        }
        reads
    } else {
        bail!("Problem getting Read or RunInfoRead elements")
    };

    let rta_version = evaluate_xpath(&info_doc, "//RTAVersion/text()")
        .chain_err(|| "Problem getting RTAVersion element")?
        .into_string();
    let rta_version3 = evaluate_xpath(&info_doc, "//RtaVersion/text()")
        .chain_err(|| "Problem getting RTAVersion element")?
        .into_string();

    Ok(RunParameters {
        planned_reads: reads,
        rta_version: if !rta_version3.is_empty() {
            rta_version3[1..].to_string()
        } else {
            rta_version
        },
        run_number: evaluate_xpath(&info_doc, "//ScanNumber/text()")
            .chain_err(|| "Problem getting ScanNumber element")?
            .into_number() as i32,
        flowcell_slot: if let Ok(elem) = evaluate_xpath(&info_doc, "//FCPosition/text()") {
            let elem = elem.into_string();
            if elem.is_empty() {
                "A".to_string()
            } else {
                elem
            }
        } else {
            "A".to_string()
        },
        experiment_name: if let Ok(elem) = evaluate_xpath(&info_doc, "//ExperimentName/text()") {
            elem.into_string()
        } else {
            "".to_string()
        },
    })
}

pub fn process_xml_param_doc_miniseq(info_doc: &Document) -> Result<RunParameters> {
    let mut reads = Vec::new();
    let mut number = 1;

    if let Ok(value) = evaluate_xpath(&info_doc, "//PlannedRead1Cycles/text()") {
        let num_cycles = value.into_number() as i32;
        if num_cycles != 0 {
            reads.push(ReadDescription {
                number: number,
                num_cycles: num_cycles,
                is_index: false,
            });
            number += 1;
        }
    }

    if let Ok(value) = evaluate_xpath(&info_doc, "//PlannedIndex1ReadCycles/text()") {
        let num_cycles = value.into_number() as i32;
        if num_cycles != 0 {
            reads.push(ReadDescription {
                number: number,
                num_cycles: num_cycles,
                is_index: true,
            });
            number += 1;
        }
    }

    if let Ok(value) = evaluate_xpath(&info_doc, "//PlannedIndex2ReadCycles/text()") {
        let num_cycles = value.into_number() as i32;
        if num_cycles != 0 {
            reads.push(ReadDescription {
                number: number,
                num_cycles: num_cycles,
                is_index: true,
            });
            number += 1;
        }
    }

    if let Ok(value) = evaluate_xpath(&info_doc, "//PlannedRead2Cycles/text()") {
        let num_cycles = value.into_number() as i32;
        if num_cycles != 0 {
            reads.push(ReadDescription {
                number: number,
                num_cycles: num_cycles,
                is_index: false,
            });
            // number += 1;
        }
    }

    let rta_version = evaluate_xpath(&info_doc, "//RTAVersion/text()")
        .chain_err(|| "Problem getting RTAVersion element")?
        .into_string();
    let rta_version3 = evaluate_xpath(&info_doc, "//RtaVersion/text()")
        .chain_err(|| "Problem getting RTAVersion element")?
        .into_string();

    Ok(RunParameters {
        planned_reads: reads,
        rta_version: if !rta_version3.is_empty() {
            rta_version3[1..].to_string()
        } else {
            rta_version
        },
        run_number: evaluate_xpath(&info_doc, "//RunNumber/text()")
            .chain_err(|| "Problem getting RunNumber element")?
            .into_number() as i32,
        flowcell_slot: if let Ok(elem) = evaluate_xpath(&info_doc, "//Side/text()") {
            let elem = elem.into_string();
            if elem.is_empty() {
                "A".to_string()
            } else {
                elem
            }
        } else {
            "A".to_string()
        },

        experiment_name: if let Ok(elem) = evaluate_xpath(&info_doc, "//ExperimentName/text()") {
            elem.into_string()
        } else {
            "".to_string()
        },
    })
}


pub fn process_xml_param_doc_novaseqxplus(info_doc: &Document) -> Result<RunParameters> {
    let mut number = 1;

    let reads = if let Value::Nodeset(nodeset) =
        evaluate_xpath(&info_doc, "//Read")
            .chain_err(|| "Problem finding PlannedReads or Read tags")?
    {
        let mut reads = Vec::new();
        for node in nodeset.document_order() {
            if let Node::Element(elem) = node {
                let num_cycles = elem
                    .attribute("Cycles")
                    .expect("Problem accessing Cycles attribute")
                    .value()
                    .to_string()
                    .parse::<i32>()
                    .unwrap();
                if num_cycles > 0 {
                    reads.push(ReadDescription {
                        number: number,
                        num_cycles: num_cycles,
                        is_index: elem
                            .attribute("ReadName")
                            .expect("Problem accessing ReadName attribute")
                            .value()
                            .to_string()
                            .starts_with("Index")
                    });
                    number += 1;
                }
            } else {
                bail!("PlannedRead or Read was not a tag!")
            }
        }
        reads
    } else {
        bail!("Problem getting Read or RunInfoRead elements")
    };

//    let rta_version3 = evaluate_xpath(&info_doc, "//RtaVersion/text()")
//        .chain_err(|| "Problem getting RTAVersion element")?
//        .into_string();
    let systemsuite_version = evaluate_xpath(&info_doc, "//SystemSuiteVersion/text()")
        .chain_err(|| "Problem getting SystemSuiteVersion element")?
        .into_string();

    Ok(RunParameters {
        planned_reads: reads,
//        rta_version: if !rta_version3.is_empty() {
//            rta_version3[1..].to_string()
//       } else {
//           systemsuite_version
//        },
        rta_version: ["3",&systemsuite_version].join("."),
        run_number: evaluate_xpath(&info_doc, "//RunNumber/text()")
            .chain_err(|| "Problem getting RunNumber element")?
            .into_number() as i32,
        flowcell_slot: if let Ok(elem) = evaluate_xpath(&info_doc, "//Side/text()") {
            let elem = elem.into_string();
            if elem.is_empty() {
                "A".to_string()
            } else {
                elem
            }
        } else {
            "A".to_string()
        },

        experiment_name: if let Ok(elem) = evaluate_xpath(&info_doc, "//ExperimentName/text()") {
            elem.into_string()
        } else {
            "".to_string()
        },
    })
}

pub fn process_xml_param_doc_nextseq2000(info_doc: &Document) -> Result<RunParameters> {
    let mut reads = Vec::new();
    let mut number = 1;

    println!("parsing NextSeq 2000 RunParameters");
    if let Ok(value) = evaluate_xpath(&info_doc, "//Read1/text()") {
        let num_cycles = value.into_number() as i32;
        if num_cycles != 0 {
            reads.push(ReadDescription {
                number: number,
                num_cycles: num_cycles,
                is_index: false,
            });
            number += 1;
        }
    } else {
        bail!("Read1 was not a tag!")
    }


    if let Ok(value) = evaluate_xpath(&info_doc, "//Index1/text()") {
        let num_cycles = value.into_number() as i32;
        if num_cycles != 0 {
            reads.push(ReadDescription {
                number: number,
                num_cycles: num_cycles,
                is_index: true,
            });
            number += 1;
        }
    } else {
        bail!("Index1 was not a tag!")
    }


    if let Ok(value) = evaluate_xpath(&info_doc, "//Index2/text()") {
        let num_cycles = value.into_number() as i32;
        if num_cycles != 0 {
            reads.push(ReadDescription {
                number: number,
                num_cycles: num_cycles,
                is_index: true,
            });
            number += 1;
        }
    } else {
        bail!("Index2 was not a tag!")
    }


    if let Ok(value) = evaluate_xpath(&info_doc, "//Read2/text()") {
        let num_cycles = value.into_number() as i32;
        if num_cycles != 0 {
            reads.push(ReadDescription {
                number: number,
                num_cycles: num_cycles,
                is_index: false,
            });
            // number += 1;
        }
    } else {
        bail!("Read2 was not a tag!")
    }


    let rta_version = evaluate_xpath(&info_doc, "//RTAVersion/text()")
        .chain_err(|| "Problem getting RTAVersion element")?
        .into_string();
    let rta_version3 = evaluate_xpath(&info_doc, "//RtaVersion/text()")
        .chain_err(|| "Problem getting RTAVersion element")?
        .into_string();

    Ok(RunParameters {
        planned_reads: reads,
        rta_version: if !rta_version3.is_empty() {
//      fix for new NextSeq2000 running RTA version 4.xxx
            if rta_version3.starts_with("4") {
                "3".to_string()
            } else {
                rta_version3.to_string()
            }
        } else {
            rta_version
        },
        run_number: evaluate_xpath(&info_doc, "//RunCounter/text()")
            .chain_err(|| "Problem getting RunNumber element")?
            .into_number() as i32,
        flowcell_slot: if let Ok(elem) = evaluate_xpath(&info_doc, "//Side/text()") {
            let elem = elem.into_string();
            if elem.is_empty() {
                "A".to_string()
            } else {
                elem
            }
        } else {
            "A".to_string()
        },

        experiment_name: if let Ok(elem) = evaluate_xpath(&info_doc, "//ExperimentName/text()") {
            elem.into_string()
        } else {
            "".to_string()
        },
    })
}


pub fn process_xml(
    logger: &slog::Logger,
    folder_layout: FolderLayout,
    info_doc: &Document,
    param_doc: &Document,
) -> Result<(RunInfo, RunParameters)> {
    let run_info = process_xml_run_info(info_doc)?;
    debug!(logger, "RunInfo => {:?}", &run_info);

    let run_params = match folder_layout {
        FolderLayout::MiSeqDep | FolderLayout:: MiSeq => process_xml_param_doc_miseq(param_doc)?,
        FolderLayout::MiniSeq | FolderLayout::NovaSeq => process_xml_param_doc_miniseq(param_doc)?,
        FolderLayout::NovaSeqXplus => process_xml_param_doc_novaseqxplus(param_doc)?,
        FolderLayout::NextSeq2000 => process_xml_param_doc_nextseq2000(param_doc)?,
        _ => bail!(
            "Don't yet know how to parse folder layout {:?}",
            folder_layout
        ),
    };
    debug!(logger, "RunParameters => {:?}", &run_params);

    Ok((run_info, run_params))
}

pub fn get_status_sequencing(
    run_info: &RunInfo,
    run_params: &RunParameters,
    path: &Path,
    current_status: &str,
) -> String {
    if current_status == "closed" || current_status == "complete" {
        // has final status
        return current_status.to_string();
    } else if (!run_params.planned_reads.is_empty()) && (run_info.reads != run_params.planned_reads)
    {
        return "failed".to_string();
    } else if path.join("RTAComplete.txt").exists() {
        return "complete".to_string();
    } else {
        return "in_progress".to_string();
    }
}
