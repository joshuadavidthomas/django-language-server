use crate::proto::v1::{self, messages};
use crate::{ProcessError, PythonProcess};

pub trait IpcCommand: Default {
    fn into_request(&self) -> messages::Request;
    fn from_response(response: messages::Response) -> Result<messages::Response, ProcessError>;

    fn execute(process: &mut PythonProcess) -> Result<messages::Response, ProcessError> {
        let cmd = Self::default();
        let request = cmd.into_request();
        let response = process.send(request).map_err(ProcessError::Transport)?;
        Self::from_response(response)
    }
}

impl IpcCommand for v1::commands::check::HealthRequest {
    fn into_request(&self) -> messages::Request {
        messages::Request {
            command: Some(messages::request::Command::CheckHealth(*self)),
        }
    }

    fn from_response(response: messages::Response) -> Result<messages::Response, ProcessError> {
        match response.result {
            Some(messages::response::Result::CheckHealth(_)) => Ok(response),
            Some(messages::response::Result::Error(e)) => Err(ProcessError::Health(e.message)),
            _ => Err(ProcessError::Response),
        }
    }
}

impl IpcCommand for v1::commands::check::GeoDjangoPrereqsRequest {
    fn into_request(&self) -> messages::Request {
        messages::Request {
            command: Some(messages::request::Command::CheckGeodjangoPrereqs(*self)),
        }
    }

    fn from_response(response: messages::Response) -> Result<messages::Response, ProcessError> {
        match response.result {
            Some(messages::response::Result::CheckGeodjangoPrereqs(_)) => Ok(response),
            Some(messages::response::Result::Error(e)) => Err(ProcessError::Health(e.message)),
            _ => Err(ProcessError::Response),
        }
    }
}

impl IpcCommand for v1::commands::python::GetEnvironmentRequest {
    fn into_request(&self) -> messages::Request {
        messages::Request {
            command: Some(messages::request::Command::PythonGetEnvironment(*self)),
        }
    }

    fn from_response(response: messages::Response) -> Result<messages::Response, ProcessError> {
        match response.result {
            Some(messages::response::Result::PythonGetEnvironment(_)) => Ok(response),
            Some(messages::response::Result::Error(e)) => Err(ProcessError::Health(e.message)),
            _ => Err(ProcessError::Response),
        }
    }
}

impl IpcCommand for v1::commands::django::GetProjectInfoRequest {
    fn into_request(&self) -> messages::Request {
        messages::Request {
            command: Some(messages::request::Command::DjangoGetProjectInfo(*self)),
        }
    }

    fn from_response(response: messages::Response) -> Result<messages::Response, ProcessError> {
        match response.result {
            Some(messages::response::Result::DjangoGetProjectInfo(_)) => Ok(response),
            Some(messages::response::Result::Error(e)) => Err(ProcessError::Health(e.message)),
            _ => Err(ProcessError::Response),
        }
    }
}
