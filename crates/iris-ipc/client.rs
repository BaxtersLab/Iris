use crate::command::IpcCommand;
use crate::response::IpcResponse;
use crate::server::IpcHandle;
use iris_core::error::IrisResult;

pub struct IpcClient {
    handle: IpcHandle,
}

impl IpcClient {
    pub fn new(handle: IpcHandle) -> Self {
        Self { handle }
    }

    pub async fn ping(&self) -> IrisResult<IpcResponse> {
        self.handle.send_command(IpcCommand::Ping).await
    }

    pub async fn get_status(&self) -> IrisResult<IpcResponse> {
        self.handle.send_command(IpcCommand::GetStatus).await
    }

    pub async fn start_capture(&self) -> IrisResult<IpcResponse> {
        self.handle.send_command(IpcCommand::StartCapture).await
    }

    pub async fn stop_capture(&self) -> IrisResult<IpcResponse> {
        self.handle.send_command(IpcCommand::StopCapture).await
    }

    pub async fn list_devices(&self) -> IrisResult<IpcResponse> {
        self.handle.send_command(IpcCommand::ListDevices).await
    }

    pub async fn select_device(&self, id: String) -> IrisResult<IpcResponse> {
        self.handle
            .send_command(IpcCommand::SelectDevice { device_id: id })
            .await
    }

    pub async fn subscribe(&self) -> IrisResult<IpcResponse> {
        self.handle.send_command(IpcCommand::Subscribe).await
    }

    pub async fn unsubscribe(&self, id: u64) -> IrisResult<IpcResponse> {
        self.handle
            .send_command(IpcCommand::Unsubscribe { subscriber_id: id })
            .await
    }
}
